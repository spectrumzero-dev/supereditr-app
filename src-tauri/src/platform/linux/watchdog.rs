//! Notices when the main thread stops answering, and logs what it was doing to capture.log. GTK runs
//! every window there, so while it's stuck the panel can't open and Hyprland soon calls SuperEditr
//! "not responding". The log names the command SuperEditr was running on it, if any, and what the
//! thread waits on in the kernel; no text.

use super::trace;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tauri::AppHandle;

/// How long the main thread may go without answering before it's logged: under Hyprland's 7.5 s
/// (five missed pings, 1.5 s apart) before it shows its dialog.
const LIMIT: Duration = Duration::from_secs(5);
const TICK: Duration = Duration::from_secs(1);

/// The command running on the main thread, and since when.
static RUNNING: Mutex<Option<(String, Instant)>> = Mutex::new(None);

/// While it's kept, a command is marked as running on the main thread; dropping it puts back the
/// one before.
pub struct RunningCommand(Option<(String, Instant)>);

impl Drop for RunningCommand {
    fn drop(&mut self) {
        *RUNNING.lock().unwrap() = self.0.take();
    }
}

/// Marks a command as running on the main thread. Commands that aren't async run there, inside the
/// invoke handler.
pub fn running_command(name: &str) -> RunningCommand {
    RunningCommand(RUNNING.lock().unwrap().replace((name.to_string(), Instant::now())))
}

/// What the watchdog reports.
#[derive(Debug, PartialEq)]
enum Event {
    Stuck(Duration),
    Answered(Duration),
}

/// The watchdog's state: when the main thread last ran its check-in, and whether the current
/// silence has been reported.
struct Watch {
    answered: Duration,
    reported: bool,
}

impl Watch {
    /// `answered` is when the main thread last checked in, `now` the time of this look; both count
    /// from the watchdog's start.
    fn look(&mut self, answered: Duration, now: Duration) -> Option<Event> {
        if answered > self.answered {
            let silent = answered.saturating_sub(self.answered);
            self.answered = answered;
            return std::mem::take(&mut self.reported).then_some(Event::Answered(silent));
        }
        let silent = now.saturating_sub(self.answered);
        (silent >= LIMIT && !self.reported).then(|| {
            self.reported = true;
            Event::Stuck(silent)
        })
    }
}

/// The calling thread's ID, from `/proc/thread-self` ("1234/task/1240").
fn thread_id() -> Option<u32> {
    let link = std::fs::read_link("/proc/thread-self").ok()?;
    link.to_str()?.rsplit('/').next()?.parse().ok()
}

/// What the thread is waiting on in the kernel: its wait channel and the system call it's in, by
/// number (x86-64: 7 poll, 202 futex, 271 ppoll, 42 connect, 0 read).
fn kernel_wait(tid: u32) -> String {
    let read = |what: &str| std::fs::read_to_string(format!("/proc/self/task/{tid}/{what}")).unwrap_or_default();
    let wchan = read("wchan");
    let syscall = read("syscall");
    let state =
        read("stat").rsplit(')').next().and_then(|rest| rest.split_whitespace().next()).unwrap_or("?").to_string();
    format!(
        "state {state}, waiting in {}, system call {}",
        if wchan.is_empty() || wchan == "0" { "nothing" } else { wchan.trim() },
        syscall.split_whitespace().next().unwrap_or("?")
    )
}

/// Starts the watchdog: once a second it asks the main thread to check in.
pub fn start(app: &AppHandle) {
    let app = app.clone();
    let _ = std::thread::Builder::new().name("main-watchdog".into()).spawn(move || {
        let started = Instant::now();
        let answered = std::sync::Arc::new(AtomicU64::new(0));
        let pending = std::sync::Arc::new(AtomicBool::new(false));
        let main_tid = std::sync::Arc::new(AtomicU64::new(0));
        let mut watch = Watch { answered: Duration::ZERO, reported: false };
        loop {
            if !pending.swap(true, Ordering::AcqRel) {
                let (answered, pending, main_tid) = (answered.clone(), pending.clone(), main_tid.clone());
                let _ = app.run_on_main_thread(move || {
                    if main_tid.load(Ordering::Relaxed) == 0 {
                        main_tid.store(thread_id().unwrap_or(0) as u64, Ordering::Relaxed);
                    }
                    answered.store(started.elapsed().as_millis() as u64, Ordering::Release);
                    pending.store(false, Ordering::Release);
                });
            }
            std::thread::sleep(TICK);
            let last = Duration::from_millis(answered.load(Ordering::Acquire));
            match watch.look(last, started.elapsed()) {
                Some(Event::Stuck(silent)) => {
                    let command = match &*RUNNING.lock().unwrap() {
                        Some((name, since)) => {
                            format!("running the command {name} for {:.1} s", since.elapsed().as_secs_f64())
                        }
                        None => "in no SuperEditr command".into(),
                    };
                    let tid = main_tid.load(Ordering::Relaxed) as u32;
                    let wait = if tid == 0 { "thread unknown".into() } else { kernel_wait(tid) };
                    trace(format_args!(
                        "main thread hasn’t answered in {:.1} s: {command}; {wait}",
                        silent.as_secs_f64()
                    ));
                }
                Some(Event::Answered(silent)) => {
                    trace(format_args!("main thread answered again after {:.1} s", silent.as_secs_f64()))
                }
                None => {}
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(seconds: f64) -> Duration {
        Duration::from_secs_f64(seconds)
    }

    #[test]
    fn a_main_thread_that_answers_is_never_reported() {
        let mut watch = Watch { answered: Duration::ZERO, reported: false };
        for second in 1..30 {
            assert_eq!(watch.look(s(second as f64 - 0.5), s(second as f64)), None);
        }
    }

    #[test]
    fn a_silence_is_reported_once_and_then_its_end() {
        let mut watch = Watch { answered: s(10.0), reported: false };
        assert_eq!(watch.look(s(10.0), s(14.0)), None);
        assert_eq!(watch.look(s(10.0), s(15.0)), Some(Event::Stuck(s(5.0))));
        assert_eq!(watch.look(s(10.0), s(40.0)), None);
        assert_eq!(watch.look(s(51.0), s(52.0)), Some(Event::Answered(s(41.0))));
        assert_eq!(watch.look(s(52.0), s(53.0)), None);
    }

    #[test]
    fn the_running_command_is_restored_when_one_runs_inside_another() {
        let outer = running_command("outer");
        {
            let _inner = running_command("inner");
            assert_eq!(RUNNING.lock().unwrap().as_ref().map(|(name, _)| name.as_str()), Some("inner"));
        }
        assert_eq!(RUNNING.lock().unwrap().as_ref().map(|(name, _)| name.as_str()), Some("outer"));
        drop(outer);
        assert!(RUNNING.lock().unwrap().is_none());
    }

    #[test]
    fn this_threads_id_and_kernel_wait_can_be_read() {
        let tid = thread_id().unwrap();
        assert!(tid > 0);
        assert!(kernel_wait(tid).starts_with("state R"));
    }
}

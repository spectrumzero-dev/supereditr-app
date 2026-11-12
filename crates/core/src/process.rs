//! Starts programs that SuperEditr doesn't ship: the Claude CLI and the user's login shell.
//!
//! macOS charges a child process's use of privacy permissions (Accessibility, Screen Recording)
//! to the app that started it, so a program SuperEditr starts can read and control other apps
//! with SuperEditr's Accessibility permission. These programs are chosen by a settings path,
//! `PATH` or `SHELL`, and the shell runs the user's startup files, so on macOS they're started
//! with `posix_spawn` and the responsibility disclaimer: macOS then treats the child as its own
//! app, with its own (usually no) permissions. Elsewhere this is `tokio::process::Command`.
//!
//! The app has no console of its own, so on Windows a console program it starts would open a
//! console window, which flashes on screen; every program started here sets `CREATE_NO_WINDOW`.

use std::io;
use std::path::Path;
use std::process::{ExitStatus, Output};
use tokio::io::{AsyncRead, AsyncReadExt};

/// Where the child's standard input comes from.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stdin {
    Piped,
    Null,
}

/// A running program with piped standard output and error. Dropping it kills the program.
pub(crate) struct Child {
    pub stdin: Option<imp::Stdin>,
    pub stdout: Option<imp::Stdout>,
    pub stderr: Option<imp::Stderr>,
    process: imp::Process,
}

impl Child {
    #[cfg(all(test, target_os = "macos"))]
    pub fn id(&self) -> u32 {
        self.process.id()
    }

    /// Closes standard input, if still open, and waits for the program to exit.
    pub async fn wait(&mut self) -> io::Result<ExitStatus> {
        drop(self.stdin.take());
        self.process.wait().await
    }

    /// Reads both outputs to the end and waits for the program to exit.
    pub async fn output(mut self) -> io::Result<Output> {
        drop(self.stdin.take());
        let (stdout, stderr) = tokio::try_join!(read_all(self.stdout.take()), read_all(self.stderr.take()))?;
        let status = self.wait().await?;
        Ok(Output { status, stdout, stderr })
    }
}

async fn read_all(pipe: Option<impl AsyncRead + Unpin>) -> io::Result<Vec<u8>> {
    let mut buffer = Vec::new();
    if let Some(mut pipe) = pipe {
        pipe.read_to_end(&mut buffer).await?;
    }
    Ok(buffer)
}

/// Starts `program` (a path, or a name looked up in `PATH`) with `args`, in `dir` if given.
pub(crate) fn spawn(program: &str, args: &[&str], dir: Option<&Path>, stdin: Stdin) -> io::Result<Child> {
    imp::spawn(program, args, dir, stdin)
}

/// Runs `program` with no input and returns everything it printed.
pub(crate) async fn output(program: &str, args: &[&str]) -> io::Result<Output> {
    spawn(program, args, None, Stdin::Null)?.output().await
}

/// Windows' `CREATE_NO_WINDOW` process creation flag.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A plain `tokio::process::Command` for a program SuperEditr ships itself (the Apple
/// Intelligence helper), which needs no disclaimer, with no console window on Windows.
pub(crate) fn command(program: impl AsRef<std::ffi::OsStr>) -> tokio::process::Command {
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut command = tokio::process::Command::new(program);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::Child;
    use std::io;
    use std::path::Path;
    use std::process::{ExitStatus, Stdio};

    pub type Stdin = tokio::process::ChildStdin;
    pub type Stdout = tokio::process::ChildStdout;
    pub type Stderr = tokio::process::ChildStderr;

    pub struct Process(tokio::process::Child);

    impl Process {
        pub async fn wait(&mut self) -> io::Result<ExitStatus> {
            self.0.wait().await
        }
    }

    pub fn spawn(program: &str, args: &[&str], dir: Option<&Path>, stdin: super::Stdin) -> io::Result<Child> {
        let mut command = super::command(program);
        command.args(args).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
        command.stdin(if stdin == super::Stdin::Piped { Stdio::piped() } else { Stdio::null() });
        if let Some(dir) = dir {
            command.current_dir(dir);
        }
        let mut child = command.spawn()?;
        Ok(Child {
            stdin: child.stdin.take(),
            stdout: child.stdout.take(),
            stderr: child.stderr.take(),
            process: Process(child),
        })
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::Child;
    use libc::{c_char, c_int};
    use std::ffi::CString;
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::process::ExitStatusExt;
    use std::path::Path;
    use std::process::ExitStatus;
    use std::sync::{Arc, Mutex, PoisonError};
    use tokio::net::unix::pipe;
    use tokio::sync::oneshot;

    pub type Stdin = pipe::Sender;
    pub type Stdout = pipe::Receiver;
    pub type Stderr = pipe::Receiver;

    unsafe extern "C" {
        // Private to libsystem since macOS 10.14, with no public replacement; Chromium and
        // LLDB start helper processes with it for the same reason.
        fn responsibility_spawnattrs_setdisclaim(attrs: *mut libc::posix_spawnattr_t, disclaim: c_int) -> c_int;
        // Public since macOS 10.15.
        fn posix_spawn_file_actions_addchdir_np(
            actions: *mut libc::posix_spawn_file_actions_t,
            path: *const c_char,
        ) -> c_int;
    }

    pub struct Process {
        pid: libc::pid_t,
        /// Set, under the lock, just before the waiter thread reaps the child; while it's
        /// false the pid still belongs to the child, so `Drop` can't kill an unrelated process.
        reaped: Arc<Mutex<bool>>,
        exit: Option<oneshot::Receiver<io::Result<ExitStatus>>>,
        status: Option<ExitStatus>,
    }

    impl Process {
        #[cfg(test)]
        pub fn id(&self) -> u32 {
            self.pid as u32
        }

        pub async fn wait(&mut self) -> io::Result<ExitStatus> {
            if let Some(status) = self.status {
                return Ok(status);
            }
            let exit = self.exit.take().ok_or_else(|| io::Error::other("the child was already waited on"))?;
            let status = exit.await.map_err(|_| io::Error::other("the child's waiter thread stopped"))??;
            self.status = Some(status);
            Ok(status)
        }
    }

    impl Drop for Process {
        fn drop(&mut self) {
            let reaped = self.reaped.lock().unwrap_or_else(PoisonError::into_inner);
            if !*reaped {
                // The waiter thread reaps it once it exits.
                unsafe { libc::kill(self.pid, libc::SIGKILL) };
            }
        }
    }

    /// Turns a `posix_spawn*` return value (an error number, not -1) into a `Result`.
    fn check(rc: c_int) -> io::Result<()> {
        if rc == 0 { Ok(()) } else { Err(io::Error::from_raw_os_error(rc)) }
    }

    fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
        let mut fds = [0; 2];
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let (read, write) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
        // So a process another thread starts meanwhile doesn't hold this pipe open.
        for fd in [&read, &write] {
            if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok((read, write))
    }

    fn c_string(bytes: &[u8]) -> io::Result<CString> {
        CString::new(bytes).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a NUL byte in an argument"))
    }

    struct Attributes(libc::posix_spawnattr_t);
    impl Drop for Attributes {
        fn drop(&mut self) {
            unsafe { libc::posix_spawnattr_destroy(&mut self.0) };
        }
    }

    struct FileActions(libc::posix_spawn_file_actions_t);
    impl Drop for FileActions {
        fn drop(&mut self) {
            unsafe { libc::posix_spawn_file_actions_destroy(&mut self.0) };
        }
    }

    pub fn spawn(program: &str, args: &[&str], dir: Option<&Path>, stdin: super::Stdin) -> io::Result<Child> {
        let file = c_string(program.as_bytes())?;
        let argv_owned = std::iter::once(Ok(file.clone()))
            .chain(args.iter().map(|a| c_string(a.as_bytes())))
            .collect::<io::Result<Vec<_>>>()?;
        let envp_owned: Vec<CString> = std::env::vars_os()
            .filter_map(|(key, value)| {
                let mut entry = key.as_bytes().to_vec();
                entry.push(b'=');
                entry.extend_from_slice(value.as_bytes());
                CString::new(entry).ok()
            })
            .collect();
        let argv: Vec<*mut c_char> =
            argv_owned.iter().map(|a| a.as_ptr().cast_mut()).chain(std::iter::once(std::ptr::null_mut())).collect();
        let envp: Vec<*mut c_char> =
            envp_owned.iter().map(|e| e.as_ptr().cast_mut()).chain(std::iter::once(std::ptr::null_mut())).collect();
        let dir = dir.map(|d| c_string(d.as_os_str().as_bytes())).transpose()?;

        let (stdin_read, stdin_write) = match stdin {
            super::Stdin::Piped => {
                let (read, write) = pipe()?;
                (read, Some(write))
            }
            super::Stdin::Null => (OwnedFd::from(std::fs::File::open("/dev/null")?), None),
        };
        let (stdout_read, stdout_write) = pipe()?;
        let (stderr_read, stderr_write) = pipe()?;

        let mut pid: libc::pid_t = 0;
        unsafe {
            let mut raw = std::mem::zeroed();
            check(libc::posix_spawnattr_init(&mut raw))?;
            let mut attributes = Attributes(raw);
            check(responsibility_spawnattrs_setdisclaim(&mut attributes.0, 1))?;
            // Like std's spawn: no blocked signals, SIGPIPE back to its default (Rust ignores
            // it, and an ignored signal stays ignored across exec), and only fds 0–2 inherited.
            let mut no_signals = std::mem::zeroed();
            libc::sigemptyset(&mut no_signals);
            check(libc::posix_spawnattr_setsigmask(&mut attributes.0, &no_signals))?;
            let mut sigpipe = std::mem::zeroed();
            libc::sigemptyset(&mut sigpipe);
            libc::sigaddset(&mut sigpipe, libc::SIGPIPE);
            check(libc::posix_spawnattr_setsigdefault(&mut attributes.0, &sigpipe))?;
            let flags = libc::POSIX_SPAWN_SETSIGMASK | libc::POSIX_SPAWN_SETSIGDEF | libc::POSIX_SPAWN_CLOEXEC_DEFAULT;
            check(libc::posix_spawnattr_setflags(&mut attributes.0, flags as libc::c_short))?;

            let mut raw = std::mem::zeroed();
            check(libc::posix_spawn_file_actions_init(&mut raw))?;
            let mut actions = FileActions(raw);
            check(libc::posix_spawn_file_actions_adddup2(&mut actions.0, stdin_read.as_raw_fd(), 0))?;
            check(libc::posix_spawn_file_actions_adddup2(&mut actions.0, stdout_write.as_raw_fd(), 1))?;
            check(libc::posix_spawn_file_actions_adddup2(&mut actions.0, stderr_write.as_raw_fd(), 2))?;
            if let Some(dir) = &dir {
                check(posix_spawn_file_actions_addchdir_np(&mut actions.0, dir.as_ptr()))?;
            }

            check(libc::posix_spawnp(
                &mut pid,
                file.as_ptr(),
                &actions.0,
                &attributes.0,
                argv.as_ptr(),
                envp.as_ptr(),
            ))?;
        }
        // The child has its own copies; the parent keeps only its ends.
        drop((stdin_read, stdout_write, stderr_write));

        let reaped = Arc::new(Mutex::new(false));
        let (exit_tx, exit) = oneshot::channel();
        let waiter_reaped = reaped.clone();
        let waiter = std::thread::Builder::new().name("supereditr-child-wait".into()).spawn(move || {
            let _ = exit_tx.send(wait_and_reap(pid, &waiter_reaped));
        });
        let process = Process { pid, reaped, exit: Some(exit), status: None };
        if let Err(e) = waiter {
            drop(process); // kills it; it stays a zombie until SuperEditr quits
            return Err(e);
        }

        Ok(Child {
            stdin: stdin_write.map(pipe::Sender::from_owned_fd).transpose()?,
            stdout: Some(pipe::Receiver::from_owned_fd(stdout_read)?),
            stderr: Some(pipe::Receiver::from_owned_fd(stderr_read)?),
            process,
        })
    }

    /// Blocks until the child exits, then reaps it.
    fn wait_and_reap(pid: libc::pid_t, reaped: &Mutex<bool>) -> io::Result<ExitStatus> {
        // WNOWAIT leaves the child unreaped, so its pid can't be reused while `Drop` might kill it.
        let exited = loop {
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            if unsafe { libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, libc::WEXITED | libc::WNOWAIT) } == 0 {
                break Ok(());
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                break Err(error);
            }
        };
        let mut reaped = reaped.lock().unwrap_or_else(PoisonError::into_inner);
        *reaped = true;
        exited?;
        let mut status = 0;
        loop {
            if unsafe { libc::waitpid(pid, &mut status, 0) } == pid {
                return Ok(ExitStatus::from_raw(status));
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use tokio::io::AsyncWriteExt;

    // The tests that run `sh`, `cat` and `pwd` need a Unix system.
    #[cfg(unix)]
    #[tokio::test]
    async fn output_captures_both_streams_and_the_exit_code() {
        // In the temp folder: a disclaimed child doesn't inherit the parent's access to a
        // protected folder (such as one on an external disk), and `sh` reports an unreadable
        // working directory on stderr.
        let script = ["-c", "cat; printf out; printf err >&2; exit 3"];
        let out = spawn("/bin/sh", &script, Some(&std::env::temp_dir()), Stdin::Null).unwrap().output().await.unwrap();
        assert_eq!(out.stdout, b"out", "stdin is empty, so cat prints nothing");
        assert_eq!(out.stderr, b"err");
        assert_eq!(out.status.code(), Some(3));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn piped_stdin_reaches_the_program_and_closes() {
        let mut child = spawn("cat", &[], None, Stdin::Piped).unwrap();
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(b"Rhyan's draft").await.unwrap();
        drop(stdin);
        let out = child.output().await.unwrap();
        assert_eq!(out.stdout, b"Rhyan's draft", "found in PATH, and saw end of input");
        assert!(out.status.success());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runs_in_the_given_directory() {
        let dir = std::env::temp_dir().canonicalize().unwrap();
        let out = spawn("/bin/pwd", &["-P"], Some(&dir), Stdin::Null).unwrap().output().await.unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), dir.display().to_string());
    }

    #[tokio::test]
    async fn a_missing_program_is_an_error() {
        let error = spawn("/nonexistent/claude", &[], None, Stdin::Null).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[cfg(target_os = "macos")]
    fn alive(pid: u32) -> bool {
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn dropping_the_child_kills_and_reaps_it() {
        let child = spawn("/bin/sleep", &["30"], None, Stdin::Null).unwrap();
        let pid = child.id();
        assert!(alive(pid));
        drop(child);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while alive(pid) {
            assert!(std::time::Instant::now() < deadline, "the child is still running or unreaped");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn the_child_is_responsible_for_itself() {
        unsafe extern "C" {
            fn responsibility_get_pid_responsible_for_pid(pid: libc::pid_t) -> libc::pid_t;
        }
        let responsible = |pid: u32| unsafe { responsibility_get_pid_responsible_for_pid(pid as libc::pid_t) } as u32;

        // A normal child is charged to whatever app this test runs under, not to itself. Some
        // hosts already start the test disclaimed, and then the two cases look the same.
        let mut ordinary = std::process::Command::new("/bin/sleep").arg("5").spawn().unwrap();
        let ordinary_owner = responsible(ordinary.id());
        let _ = ordinary.kill();
        let _ = ordinary.wait();
        if ordinary_owner == ordinary.id() {
            eprintln!("skipped: this host doesn't pass responsibility to an ordinary child");
            return;
        }

        let child = spawn("/bin/sleep", &["5"], None, Stdin::Null).unwrap();
        assert_eq!(responsible(child.id()), child.id(), "a disclaimed child answers for its own permissions");
    }
}

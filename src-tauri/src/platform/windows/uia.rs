//! UI Automation, on a worker thread of its own.
//!
//! One thread in COM's multithreaded apartment owns every UI Automation object; none leaves it.
//! Callers send it a job with a deadline and get plain data back:
//!
//! - UI Automation's own `ConnectionTimeout` and `TransactionTimeout` are set to 2 s, so a hung app
//!   fails a call instead of blocking it (one call can still take about four times that, one wait
//!   per process hop).
//! - A caller waits only until its deadline. A job still queued when its deadline passes is never
//!   run, so a late selection change can't happen after its review ended.
//! - One job runs at a time. A worker still inside a call past its deadline is wedged: it refuses
//!   new work until the call returns. One wedged for longer than UI Automation's timeouts allow is
//!   replaced, a few times at most, after which UI Automation is reported unavailable.
//!
//! [`Worker`] is generic over the state its thread owns, so it can be tested with a fake that
//! hangs.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallError {
    /// The answer didn't come by the deadline.
    Timeout,
    /// The worker is still inside an earlier call.
    Wedged,
    /// UI Automation couldn't start, or kept hanging.
    Unavailable,
}

type Job<C> = Box<dyn FnOnce(&mut C) + Send>;

struct Thread<C> {
    jobs: Sender<Job<C>>,
    /// When the call it's running began; None while idle.
    busy: Arc<Mutex<Option<Instant>>>,
}

pub struct Worker<C: 'static> {
    make: Arc<dyn Fn() -> Option<C> + Send + Sync>,
    thread: Mutex<Option<Thread<C>>>,
    replaced: Mutex<u32>,
    /// One call at a time.
    calls: Mutex<()>,
    generation: AtomicU64,
    /// A call running longer than this is taken to be stuck, and its thread is replaced.
    stuck_after: Duration,
    max_replacements: u32,
}

impl<C: 'static> Worker<C> {
    /// `make` builds the thread's state on the thread itself.
    pub fn new(make: impl Fn() -> Option<C> + Send + Sync + 'static, stuck_after: Duration) -> Self {
        Worker {
            make: Arc::new(make),
            thread: Mutex::new(None),
            replaced: Mutex::new(0),
            calls: Mutex::new(()),
            generation: AtomicU64::new(0),
            stuck_after,
            max_replacements: 3,
        }
    }

    fn spawn(&self) -> Thread<C> {
        let (jobs, inbox): (Sender<Job<C>>, Receiver<Job<C>>) = channel();
        let busy = Arc::new(Mutex::new(None));
        let make = self.make.clone();
        let running = busy.clone();
        let _ = std::thread::Builder::new().name("ui automation".into()).spawn(move || {
            let Some(mut state) = make() else { return };
            for job in inbox {
                *running.lock().unwrap() = Some(Instant::now());
                job(&mut state);
                *running.lock().unwrap() = None;
            }
        });
        Thread { jobs, busy }
    }

    /// The number of the latest call, which a caller can keep to know whether it's still current.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// Runs `job` on the worker and waits up to `limit` for its answer.
    pub fn call<R: Send + 'static>(
        &self,
        limit: Duration,
        job: impl FnOnce(&mut C, u64) -> R + Send + 'static,
    ) -> Result<R, CallError> {
        let _one = self.calls.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let deadline = Instant::now() + limit;
        let jobs = {
            let mut thread = self.thread.lock().unwrap();
            if let Some(current) = thread.as_ref() {
                let since = *current.busy.lock().unwrap();
                match since {
                    Some(at) if at.elapsed() < self.stuck_after => return Err(CallError::Wedged),
                    Some(_) => {
                        let mut replaced = self.replaced.lock().unwrap();
                        if *replaced >= self.max_replacements {
                            return Err(CallError::Unavailable);
                        }
                        *replaced += 1;
                        // The stuck thread finishes its call and then exits: its queue is closed.
                        *thread = None;
                    }
                    None => {}
                }
            }
            thread.get_or_insert_with(|| self.spawn()).jobs.clone()
        };
        let (answer, answers) = channel();
        let queued = jobs.send(Box::new(move |state: &mut C| {
            // Too late: whoever asked has stopped waiting, and must not be surprised by a side effect.
            if Instant::now() >= deadline {
                return;
            }
            let _ = answer.send(job(state, generation));
        }));
        if queued.is_err() {
            *self.thread.lock().unwrap() = None;
            return Err(CallError::Unavailable);
        }
        match answers.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(result) => Ok(result),
            Err(RecvTimeoutError::Timeout) => Err(CallError::Timeout),
            // The job was dropped: too late to run, or the thread couldn't start.
            Err(RecvTimeoutError::Disconnected) => {
                if Instant::now() >= deadline {
                    Err(CallError::Timeout)
                } else {
                    *self.thread.lock().unwrap() = None;
                    Err(CallError::Unavailable)
                }
            }
        }
    }
}

/// A screen rectangle in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PxRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl PxRect {
    pub fn is_empty(&self) -> bool {
        self.right <= self.left || self.bottom <= self.top
    }
}

/// What a read of the focused element found. Text is UTF-16, as UI Automation counts it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Field {
    /// Which stored element and range on the worker this read belongs to.
    pub token: u64,
    pub pid: u32,
    pub control_type: i32,
    /// The field's UI Automation runtime ID, which tells it apart from the other elements alive in
    /// its window. Windows can give it to another element once this one is destroyed, so it says
    /// "the same field" only together with the window and the selection. Empty when unavailable.
    pub runtime_id: Vec<i32>,
    pub class: String,
    pub framework: String,
    pub name: String,
    /// The element or one of its parents is a password field.
    pub password: bool,
    /// The selection's `IsReadOnly`: `Some(false)` only when UI Automation said false. Mixed, not
    /// supported or a timed-out answer (which comes back as success with no boolean) are `None`.
    pub read_only: Option<bool>,
    /// An input method is composing text in the field.
    pub composition: bool,
    /// How many separate ranges are selected.
    pub ranges: usize,
    pub text: Vec<u16>,
    /// The selection's offsets in the document.
    pub start: usize,
    pub end: usize,
    /// Up to `CONTEXT` units of text on either side of the selection.
    pub before: Vec<u16>,
    pub after: Vec<u16>,
    pub bounds: Option<PxRect>,
    pub element_bounds: Option<PxRect>,
}

/// Units of text kept on each side of a selection, to tell two identical sentences apart.
pub const CONTEXT: usize = 40;

/// The selection now, compared with a stored read.
#[derive(Debug, Clone, PartialEq)]
pub struct Recheck {
    /// The focused element is the stored one (`CompareElements`).
    pub same_element: bool,
    pub ranges: usize,
    /// Both ends of the selection equal the stored range's (`CompareEndpoints`).
    pub same_endpoints: bool,
    pub text: Vec<u16>,
    pub before: Vec<u16>,
    pub after: Vec<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadError {
    /// The focused element is a password field, or inside one.
    Password,
    /// The focused element has no text SuperEditr can read: no element with a TextPattern, or one
    /// that hasn't reported its selection yet.
    NoText,
    /// The stored element isn't focused any more, or its read has been replaced.
    Gone,
    Call(CallError),
    Failed(String),
}

impl From<CallError> for ReadError {
    fn from(error: CallError) -> Self {
        ReadError::Call(error)
    }
}

/// What the capture and replace chains ask of UI Automation.
pub trait Fields: Send + Sync {
    /// One read of the focused element and its selection, stored under a new token.
    fn focused(&self, limit: Duration) -> Result<Field, ReadError>;
    /// The selection now, against the stored read `token`.
    fn recheck(&self, token: u64, limit: Duration) -> Result<Recheck, ReadError>;
    /// The whole text of the focused element, read through a freshly fetched element that must
    /// compare equal to the stored one: a Win32 edit control's stored element keeps returning its
    /// old text after a paste of the same length.
    fn document(&self, token: u64, limit: Duration) -> Result<Vec<u16>, ReadError>;
    /// Whether the stored element has keyboard focus again.
    fn focused_is(&self, token: u64, limit: Duration) -> bool;
}

/// The worker's state on Windows.
pub mod system {
    use super::{CONTEXT, CallError, Field, Fields, PxRect, ReadError, Recheck, Worker};
    use std::time::Duration;
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    };
    use windows::Win32::System::Ole::{SafeArrayDestroy, SafeArrayGetElement, SafeArrayGetLBound, SafeArrayGetUBound};
    use windows::Win32::UI::Accessibility::*;
    use windows::core::Interface;

    /// UI Automation's per-hop timeouts.
    const TIMEOUT_MS: u32 = 2000;
    /// How far up from the focused element to look for the editor holding the text: Chromium and
    /// Electron often report a part of the field as focused.
    const PARENTS: usize = 10;
    /// The longest document read whole.
    const MAX_TEXT: i32 = 4_000_000;

    struct Stored {
        token: u64,
        focused: IUIAutomationElement,
        editor: IUIAutomationElement,
        range: IUIAutomationTextRange,
    }

    pub struct Uia {
        automation: IUIAutomation,
        walker: Option<IUIAutomationTreeWalker>,
        stored: Option<Stored>,
    }

    impl Uia {
        fn new() -> Option<Self> {
            unsafe {
                CoInitializeEx(None, COINIT_MULTITHREADED).ok().ok()?;
                let automation: IUIAutomation = CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER).ok()?;
                if let Ok(timeouts) = automation.cast::<IUIAutomation2>() {
                    let _ = timeouts.SetConnectionTimeout(TIMEOUT_MS);
                    let _ = timeouts.SetTransactionTimeout(TIMEOUT_MS);
                }
                let walker = automation.ControlViewWalker().ok();
                Some(Uia { automation, walker, stored: None })
            }
        }

        fn parent(&self, element: &IUIAutomationElement) -> Option<IUIAutomationElement> {
            unsafe { self.walker.as_ref()?.GetParentElement(element).ok() }
        }

        /// The focused element or its nearest parent, in the same process, with a TextPattern.
        fn editor(&self, focused: &IUIAutomationElement) -> Option<(IUIAutomationElement, IUIAutomationTextPattern)> {
            let pid = unsafe { focused.CurrentProcessId() }.unwrap_or(0);
            let mut current = Some(focused.clone());
            for _ in 0..PARENTS {
                let element = current.take()?;
                if let Ok(pattern) =
                    unsafe { element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) }
                {
                    return Some((element, pattern));
                }
                current = self.parent(&element).filter(|p| unsafe { p.CurrentProcessId() }.unwrap_or(-1) == pid);
            }
            None
        }

        /// Whether the element or a parent in its process says it's a password field.
        fn password(&self, focused: &IUIAutomationElement) -> bool {
            let pid = unsafe { focused.CurrentProcessId() }.unwrap_or(0);
            let mut current = Some(focused.clone());
            for _ in 0..PARENTS {
                let Some(element) = current.take() else { return false };
                if unsafe { element.CurrentIsPassword() }.is_ok_and(|b| b.as_bool()) {
                    return true;
                }
                current = self.parent(&element).filter(|p| unsafe { p.CurrentProcessId() }.unwrap_or(-1) == pid);
            }
            false
        }

        fn read_only(&self, range: &IUIAutomationTextRange) -> Option<bool> {
            let value = unsafe { range.GetAttributeValue(UIA_IsReadOnlyAttributeId) }.ok()?;
            // Anything but a boolean (mixed, not supported, or a timed-out call that reports
            // success) is unknown, never "not read-only".
            bool::try_from(&value).ok()
        }

        fn composing(&self, focused: &IUIAutomationElement, editor: &IUIAutomationElement) -> bool {
            [focused, editor].into_iter().any(|element| unsafe {
                element
                    .GetCurrentPatternAs::<IUIAutomationTextEditPattern>(UIA_TextEditPatternId)
                    .and_then(|pattern| pattern.GetActiveComposition())
                    .and_then(|range| range.GetText(16))
                    .is_ok_and(|text| !text.is_empty())
            })
        }

        fn runtime_id(&self, element: &IUIAutomationElement) -> Vec<i32> {
            unsafe {
                let Ok(array) = element.GetRuntimeId() else { return Vec::new() };
                if array.is_null() {
                    return Vec::new();
                }
                let mut ids = Vec::new();
                if let (Ok(low), Ok(high)) = (SafeArrayGetLBound(array, 1), SafeArrayGetUBound(array, 1)) {
                    for index in low..=high {
                        let mut id = 0i32;
                        if SafeArrayGetElement(array, &index, (&mut id as *mut i32).cast()).is_ok() {
                            ids.push(id);
                        }
                    }
                }
                let _ = SafeArrayDestroy(array);
                ids
            }
        }

        fn first_rect(&self, range: &IUIAutomationTextRange) -> Option<PxRect> {
            unsafe {
                let array = range.GetBoundingRectangles().ok()?;
                if array.is_null() {
                    return None;
                }
                let mut rects = std::ptr::null_mut();
                let n = self.automation.SafeArrayToRectNativeArray(array, &mut rects).unwrap_or(0);
                let mut union: Option<PxRect> = None;
                for i in 0..n.max(0) as usize {
                    let r = *rects.add(i);
                    let r = PxRect { left: r.left, top: r.top, right: r.right, bottom: r.bottom };
                    if r.is_empty() {
                        continue;
                    }
                    // The first line, widened by the lines below it.
                    union = Some(match union {
                        None => r,
                        Some(u) => PxRect { left: u.left.min(r.left), right: u.right.max(r.right), ..u },
                    });
                }
                if !rects.is_null() {
                    CoTaskMemFree(Some(rects as *const _));
                }
                let _ = SafeArrayDestroy(array);
                union
            }
        }

        fn focused_element(&self) -> Result<IUIAutomationElement, ReadError> {
            unsafe { self.automation.GetFocusedElement() }.map_err(|e| ReadError::Failed(format!("{:#x}", e.code().0)))
        }

        fn same(&self, a: &IUIAutomationElement, b: &IUIAutomationElement) -> bool {
            unsafe { self.automation.CompareElements(a, b) }.is_ok_and(|same| same.as_bool())
        }

        fn read(&mut self, token: u64) -> Result<Field, ReadError> {
            let focused = self.focused_element()?;
            let password = self.password(&focused);
            if password {
                return Err(ReadError::Password);
            }
            let (editor, pattern) = self.editor(&focused).ok_or(ReadError::NoText)?;
            let selection = unsafe { pattern.GetSelection() }.map_err(|_| ReadError::NoText)?;
            let ranges = unsafe { selection.Length() }.unwrap_or(0).max(0) as usize;
            // Firefox reports the page document, with no selection, for a field that was focused
            // before SuperEditr first asked.
            if ranges == 0 {
                return Err(ReadError::NoText);
            }
            let range = unsafe { selection.GetElement(0) }.map_err(|_| ReadError::NoText)?;
            let text = text_of(&range).ok_or(ReadError::NoText)?;
            let document = unsafe { pattern.DocumentRange() }.map_err(|_| ReadError::NoText)?;
            let (start, end) = offsets(&document, &range).ok_or(ReadError::NoText)?;
            let whole = text_of(&document).ok_or(ReadError::NoText)?;
            let (before, after) = context(&whole, start, end);
            let element = |e: &IUIAutomationElement| unsafe {
                (
                    e.CurrentControlType().map(|c| c.0).unwrap_or(0),
                    e.CurrentClassName().map(|s| s.to_string()).unwrap_or_default(),
                    e.CurrentFrameworkId().map(|s| s.to_string()).unwrap_or_default(),
                    e.CurrentName().map(|s| s.to_string()).unwrap_or_default(),
                )
            };
            let (control_type, class, framework, name) = element(&focused);
            let field = Field {
                token,
                pid: unsafe { focused.CurrentProcessId() }.unwrap_or(0) as u32,
                control_type,
                runtime_id: self.runtime_id(&editor),
                class,
                framework,
                name,
                password,
                read_only: self.read_only(&range),
                composition: self.composing(&focused, &editor),
                ranges,
                text,
                start,
                end,
                before,
                after,
                bounds: self.first_rect(&range),
                element_bounds: unsafe { editor.CurrentBoundingRectangle() }
                    .ok()
                    .map(|r| PxRect { left: r.left, top: r.top, right: r.right, bottom: r.bottom })
                    .filter(|r| !r.is_empty()),
            };
            self.stored = Some(Stored { token, focused, editor, range });
            Ok(field)
        }

        fn stored(&self, token: u64) -> Result<&Stored, ReadError> {
            self.stored.as_ref().filter(|s| s.token == token).ok_or(ReadError::Gone)
        }

        fn recheck(&self, token: u64) -> Result<Recheck, ReadError> {
            let stored = self.stored(token)?;
            let now = self.focused_element()?;
            let same_element = self.same(&now, &stored.focused);
            let pattern = unsafe { stored.editor.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) }
                .map_err(|_| ReadError::NoText)?;
            let selection = unsafe { pattern.GetSelection() }.map_err(|_| ReadError::NoText)?;
            let ranges = unsafe { selection.Length() }.unwrap_or(0).max(0) as usize;
            if ranges != 1 {
                return Ok(Recheck {
                    same_element,
                    ranges,
                    same_endpoints: false,
                    text: Vec::new(),
                    before: Vec::new(),
                    after: Vec::new(),
                });
            }
            let range = unsafe { selection.GetElement(0) }.map_err(|_| ReadError::NoText)?;
            let compare = |end: TextPatternRangeEndpoint| unsafe { range.CompareEndpoints(end, &stored.range, end) };
            let same_endpoints = compare(TextPatternRangeEndpoint_Start).is_ok_and(|c| c == 0)
                && compare(TextPatternRangeEndpoint_End).is_ok_and(|c| c == 0);
            let text = text_of(&range).unwrap_or_default();
            let document = unsafe { pattern.DocumentRange() }.map_err(|_| ReadError::NoText)?;
            let (before, after) = match (offsets(&document, &range), text_of(&document)) {
                (Some((start, end)), Some(whole)) => context(&whole, start, end),
                _ => (Vec::new(), Vec::new()),
            };
            Ok(Recheck { same_element, ranges, same_endpoints, text, before, after })
        }

        fn document(&self, token: u64) -> Result<Vec<u16>, ReadError> {
            let stored = self.stored(token)?;
            let now = self.focused_element()?;
            if !self.same(&now, &stored.focused) {
                return Err(ReadError::Gone);
            }
            let (_, pattern) = self.editor(&now).ok_or(ReadError::NoText)?;
            let document = unsafe { pattern.DocumentRange() }.map_err(|_| ReadError::NoText)?;
            text_of(&document).ok_or(ReadError::NoText)
        }

        fn focused_is(&self, token: u64) -> bool {
            let Ok(stored) = self.stored(token) else { return false };
            self.focused_element().is_ok_and(|now| self.same(&now, &stored.focused))
        }
    }

    fn text_of(range: &IUIAutomationTextRange) -> Option<Vec<u16>> {
        unsafe { range.GetText(MAX_TEXT) }.ok().map(|text| text.to_string().encode_utf16().collect())
    }

    /// UTF-16 offsets of a range's ends in the document.
    fn offsets(document: &IUIAutomationTextRange, range: &IUIAutomationTextRange) -> Option<(usize, usize)> {
        let upto = |end: TextPatternRangeEndpoint| unsafe {
            let prefix = document.Clone().ok()?;
            prefix.MoveEndpointByRange(TextPatternRangeEndpoint_End, range, end).ok()?;
            text_of(&prefix).map(|t| t.len())
        };
        let start = upto(TextPatternRangeEndpoint_Start)?;
        let end = upto(TextPatternRangeEndpoint_End)?;
        Some((start, end.max(start)))
    }

    fn context(whole: &[u16], start: usize, end: usize) -> (Vec<u16>, Vec<u16>) {
        let start = start.min(whole.len());
        let end = end.clamp(start, whole.len());
        (whole[start.saturating_sub(CONTEXT)..start].to_vec(), whole[end..(end + CONTEXT).min(whole.len())].to_vec())
    }

    /// UI Automation on its worker.
    pub struct System {
        worker: Worker<Uia>,
    }

    impl System {
        pub fn new() -> Self {
            // About four of UI Automation's per-hop waits, after which a call can't still be
            // legitimately running.
            System { worker: Worker::new(Uia::new, Duration::from_millis(u64::from(TIMEOUT_MS) * 4 + 500)) }
        }

        /// Makes the first contact early: Chromium builds its accessibility tree when a client
        /// first asks.
        pub fn warm(&self) {
            let _ = self.worker.call(Duration::from_secs(3), |uia, _| {
                let _ = uia.focused_element();
            });
        }

        /// Asks for the focused element of a window that just came to the front, without waiting
        /// long: Firefox takes several seconds to turn its accessibility on, and Chromium builds its
        /// tree on the first request, so the first review there doesn't pay for it.
        pub fn touch(&self) {
            let _ = self.worker.call(Duration::from_millis(300), |uia, _| {
                let _ = uia.focused_element();
            });
        }
    }

    fn flatten<T>(result: Result<Result<T, ReadError>, CallError>) -> Result<T, ReadError> {
        result.map_err(ReadError::Call).and_then(|inner| inner)
    }

    impl Fields for System {
        fn focused(&self, limit: Duration) -> Result<Field, ReadError> {
            flatten(self.worker.call(limit, |uia, generation| uia.read(generation)))
        }
        fn recheck(&self, token: u64, limit: Duration) -> Result<Recheck, ReadError> {
            flatten(self.worker.call(limit, move |uia, _| uia.recheck(token)))
        }
        fn document(&self, token: u64, limit: Duration) -> Result<Vec<u16>, ReadError> {
            flatten(self.worker.call(limit, move |uia, _| uia.document(token)))
        }
        fn focused_is(&self, token: u64, limit: Duration) -> bool {
            self.worker.call(limit, move |uia, _| uia.focused_is(token)).unwrap_or(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// A stand-in for UI Automation: each job may sleep, as a hung app makes a call do.
    #[derive(Default)]
    struct Fake {
        calls: Arc<AtomicUsize>,
    }

    fn worker(calls: Arc<AtomicUsize>, stuck_after: Duration) -> Worker<Fake> {
        Worker::new(move || Some(Fake { calls: calls.clone() }), stuck_after)
    }

    #[test]
    fn an_answer_in_time_is_returned_with_its_generation() {
        let w = worker(Arc::default(), Duration::from_secs(5));
        assert_eq!(w.call(Duration::from_secs(1), |_, generation| generation), Ok(1));
        assert_eq!(w.call(Duration::from_secs(1), |_, generation| generation), Ok(2));
        assert_eq!(w.generation(), 2);
    }

    #[test]
    fn a_hung_call_times_out_and_the_worker_refuses_work_while_wedged() {
        let calls = Arc::new(AtomicUsize::new(0));
        let w = worker(calls.clone(), Duration::from_secs(5));
        let started = Instant::now();
        let hung = w.call(Duration::from_millis(50), |fake, _| {
            fake.calls.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(300));
        });
        assert_eq!(hung, Err(CallError::Timeout));
        assert!(started.elapsed() < Duration::from_millis(250), "the caller waits only until its deadline");
        assert_eq!(w.call(Duration::from_millis(50), |_, _| ()), Err(CallError::Wedged));
        std::thread::sleep(Duration::from_millis(350));
        assert_eq!(w.call(Duration::from_millis(200), |_, _| 7), Ok(7), "usable again once the call returned");
    }

    #[test]
    fn a_job_whose_deadline_passed_in_the_queue_never_runs() {
        let calls = Arc::new(AtomicUsize::new(0));
        let ran_late = Arc::new(AtomicUsize::new(0));
        let w = Arc::new(worker(calls.clone(), Duration::from_secs(5)));
        // Occupy the worker directly, so the next job waits in its queue.
        let jobs = w.thread.lock().unwrap().get_or_insert_with(|| w.spawn()).jobs.clone();
        jobs.send(Box::new(|_: &mut Fake| std::thread::sleep(Duration::from_millis(200)))).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        *w.thread.lock().unwrap().as_ref().unwrap().busy.lock().unwrap() = None; // seen as idle
        let late = ran_late.clone();
        let result = w.call(Duration::from_millis(50), move |_, _| {
            late.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(result, Err(CallError::Timeout));
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(ran_late.load(Ordering::SeqCst), 0, "a selection change after its review ended never happens");
    }

    #[test]
    fn a_worker_stuck_too_long_is_replaced_a_few_times_then_reported_unavailable() {
        let w = worker(Arc::default(), Duration::from_millis(60));
        for _ in 0..3 {
            let _ = w.call(Duration::from_millis(10), |_, _| std::thread::sleep(Duration::from_millis(400)));
            std::thread::sleep(Duration::from_millis(80));
            // Replaced: a fresh thread answers at once.
            assert_eq!(w.call(Duration::from_millis(100), |_, _| 1), Ok(1));
        }
        let _ = w.call(Duration::from_millis(10), |_, _| std::thread::sleep(Duration::from_millis(400)));
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(w.call(Duration::from_millis(100), |_, _| 1), Err(CallError::Unavailable));
    }

    #[test]
    fn a_state_that_cant_be_built_is_unavailable() {
        let w: Worker<Fake> = Worker::new(|| None, Duration::from_secs(1));
        assert_eq!(w.call(Duration::from_millis(200), |_, _| 1), Err(CallError::Unavailable));
    }
}

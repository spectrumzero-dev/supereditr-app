use crate::platform::{Capture, Rect};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};
use supereditr_core::detect::Candidate;
use supereditr_core::provider::{ProviderKind, ProviderProfile};
use supereditr_core::{Config, Engine};

pub struct AppState {
    pub config: RwLock<Config>,
    pub config_path: PathBuf,
    pub engine: Engine,
    /// The review or notice on screen, and the quiet period after a fix.
    ///
    /// The main thread takes this lock: `review::on_review_hotkey`, run there by the global shortcut
    /// and the tray menu, takes it to hand the keys to an automatic review, and so do the
    /// synchronous commands (`dismiss`, `focus_overlay`, `resize_overlay`). Whoever holds it
    /// therefore holds up the main thread, so it is held only to read or change `ReviewState`.
    ///
    /// While holding it, never `.await`, never emit, never wait for the main thread (it may be
    /// waiting for this lock), and never make a platform call that can wait on another app, except
    /// `follow`'s reads of the watched window. It isn't reentrant: never call anything that takes it, such as
    /// `follow::hold_for_click`, `follow::gesture_ended` or `review::dismiss`. A review that ends is
    /// handed back by `ReviewState` and dropped after the lock, as dropping it stops watching its
    /// window through the platform.
    pub review: Mutex<ReviewState>,
    pub next_id: AtomicU64,
    /// Results of the last account scan, keys included, until the user picks one.
    pub candidates: Mutex<Vec<Candidate>>,
    /// API keys read from the keychain, cached so a review never waits on Keychain Services.
    keys: Mutex<HashMap<String, Option<String>>>,
    pub paused: AtomicBool,
    /// Counts the selection watcher's events, so a review about to start can tell that a newer
    /// event replaced it. Lock-free: every keystroke and click in another app counts it on.
    pub watch_generation: AtomicU64,
    /// The last finished review, which reopening the panel on the same selection shows again.
    pub last_review: crate::last_review::Memory,
    /// Counts saves of the settings, so Settings' save of a copy read before another save (the menu
    /// bar's "Turn off in …", the panel's "Always keep") can be refused instead of undoing it.
    revision: AtomicU64,
}

pub struct Session {
    pub id: u64,
    pub capture: Capture,
    /// A review the user opened (or switched to the keyboard): the panel takes the review keys.
    pub keyboard: bool,
    pub anchor: Option<Rect>,
    pub height: f64,
    /// The engine's latest run for the review, which `cancel` stops.
    pub task: Option<Task>,
    /// The number of the latest run the panel asked for: 0 for the one the session starts with,
    /// then one more for each run the panel asks for. Its engine events carry it.
    pub run: u64,
    /// The latest run is a plain proofread (no other style, no instruction), which is applied in
    /// place where the app allows it.
    pub proofread: bool,
    /// The window the text is in, which the panel follows; dropping it stops watching that window.
    pub following: Option<crate::follow::Following>,
}

impl Session {
    pub fn cancel(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// Work running for a review on a task of its own, such as the engine's run. Dropping it leaves the
/// work running; `abort` stops it.
pub struct Task(Box<dyn FnOnce() + Send>);

impl Task {
    /// The task that `abort` stops.
    pub fn new(abort: impl FnOnce() + Send + 'static) -> Self {
        Self(Box::new(abort))
    }

    pub fn abort(self) {
        (self.0)()
    }
}

/// How long after a fix is written into an app its own selection changes are ignored.
pub const QUIET: Duration = Duration::from_millis(800);

/// What is on screen for a review, and the rules for changing it. It sits behind one lock
/// (`AppState::review`); each method changes only this data and returns what the caller does once
/// the lock is released, such as dropping the review that ended, which stops watching its window.
pub struct ReviewState {
    session: Option<Session>,
    /// A standalone notice (no review) is on screen.
    notice: bool,
    /// Selection events are ignored until this moment (just after we replaced text).
    quiet_until: Instant,
}

/// What `ReviewState::dismiss` asks of the caller. Only ever a return value, so its size is no cost.
#[allow(clippy::large_enum_variant)]
pub enum Dismissal {
    /// The review asked about has ended, or another replaced it: nothing closes.
    NotCurrent,
    /// The panel closes. Holds the review that ended, if one was showing, to drop after the lock.
    Close(Option<Session>),
}

/// A review the user accepted, for writing its text back into the app.
pub struct Applying {
    pub capture: Capture,
    /// The latest run is a plain proofread (`Session::proofread`).
    pub proofread: bool,
    /// The review's watch on its window, which ends now: to drop after the lock.
    pub following: Option<crate::follow::Following>,
}

impl ReviewState {
    pub fn new(now: Instant) -> Self {
        Self { session: None, notice: false, quiet_until: now }
    }

    pub fn session(&self) -> Option<&Session> {
        self.session.as_ref()
    }

    pub fn session_mut(&mut self) -> Option<&mut Session> {
        self.session.as_mut()
    }

    /// The review numbered `id`, if it is still the one showing.
    pub fn current(&self, id: u64) -> Option<&Session> {
        self.session.as_ref().filter(|s| s.id == id)
    }

    pub fn current_mut(&mut self, id: u64) -> Option<&mut Session> {
        self.session.as_mut().filter(|s| s.id == id)
    }

    /// Opens `session`, replacing any review or notice showing. Returns the review it replaced,
    /// cancelled.
    pub fn begin(&mut self, session: Session) -> Option<Session> {
        let old = self.take_cancelled();
        self.notice = false;
        self.session = Some(session);
        old
    }

    /// A standalone notice replaces whatever is showing. Returns the review it replaced, cancelled.
    pub fn show_notice(&mut self) -> Option<Session> {
        let old = self.take_cancelled();
        self.notice = true;
        old
    }

    /// The panel asks for run number `run` of review `id`. Returns the review, its earlier run
    /// cancelled, when the run is current; None when the review has ended or a later run was
    /// asked for already.
    pub fn start_run(&mut self, id: u64, run: u64, proofread: bool) -> Option<&mut Session> {
        let current = self.session.as_mut().filter(|s| s.id == id && run >= s.run)?;
        current.cancel();
        current.run = run;
        current.proofread = proofread;
        Some(current)
    }

    /// The user accepted review `id`: its run stops and the panel closes for good, though the review
    /// stays until the text is written (`end`). None when the review has ended.
    pub fn apply(&mut self, id: u64) -> Option<Applying> {
        let current = self.current_mut(id)?;
        current.cancel();
        Some(Applying {
            capture: current.capture.clone(),
            proofread: current.proofread,
            following: current.following.take(),
        })
    }

    /// Review `id`'s text has been written (or wasn't): it ends, as it is. A review that began
    /// meanwhile stays. Returns the review that ended, if `id` was still the one showing.
    pub fn end(&mut self, id: u64) -> Option<Session> {
        self.session.take_if(|s| s.id == id)
    }

    /// Review `id` comes back after a replacement that didn't write, and takes the keys. Swaps
    /// `following` in, leaving the review's earlier one there to drop after the lock; on false the
    /// review has ended, and `following` is left as it was.
    pub fn reopen(&mut self, id: u64, following: &mut Option<crate::follow::Following>) -> bool {
        let Some(current) = self.current_mut(id) else { return false };
        std::mem::swap(&mut current.following, following);
        current.keyboard = true;
        true
    }

    /// Closes the panel: review `id`, or with None whatever is showing.
    pub fn dismiss(&mut self, id: Option<u64>) -> Dismissal {
        if id.is_some() && self.session.as_ref().map(|s| s.id) != id {
            return Dismissal::NotCurrent;
        }
        let ended = self.take_cancelled();
        self.notice = false;
        Dismissal::Close(ended)
    }

    /// The panel takes the keys. Returns the review's number when it was an automatic one until now.
    pub fn take_keyboard(&mut self) -> Option<u64> {
        let current = self.session.as_mut()?;
        let was_automatic = !current.keyboard;
        current.keyboard = true;
        was_automatic.then_some(current.id)
    }

    /// The panel's content is `height` tall. Returns where the review's selection is, if known.
    pub fn resize(&mut self, height: f64) -> Option<Rect> {
        let current = self.session.as_mut()?;
        current.height = height;
        current.anchor
    }

    /// A fix is being written into the app: its own selection changes are ignored for `QUIET`.
    pub fn start_quiet(&mut self, now: Instant) {
        self.quiet_until = now + QUIET;
    }

    pub fn is_quiet(&self, now: Instant) -> bool {
        now < self.quiet_until
    }

    /// A review or a notice is on screen.
    pub fn showing(&self) -> bool {
        self.session.is_some() || self.notice
    }

    pub fn notice_visible(&self) -> bool {
        self.notice
    }

    /// Whether typing or a click in another app closes what is showing (unless it was a click on
    /// the reviewed window): not during the quiet period after a fix.
    pub fn closes_on_interaction(&self, now: Instant) -> bool {
        self.showing() && !self.is_quiet(now)
    }

    /// The process whose text the review showing is of.
    pub fn reviewed_pid(&self) -> Option<i32> {
        self.session.as_ref().map(|s| s.capture.pid)
    }

    /// A review that has the keys is showing, which a new selection never replaces.
    pub fn keyboard_review_open(&self) -> bool {
        self.session.as_ref().is_some_and(|s| s.keyboard)
    }

    /// The review showing is of exactly `text`.
    pub fn is_showing_text(&self, text: &str) -> bool {
        self.session.as_ref().is_some_and(|s| s.capture.text == text)
    }

    fn take_cancelled(&mut self) -> Option<Session> {
        let mut old = self.session.take()?;
        old.cancel();
        Some(old)
    }
}

impl AppState {
    pub fn new(config_path: PathBuf) -> Self {
        let config = Config::load(&config_path);
        crate::platform::set_trace(config.diagnostic_log);
        let engine = Engine::new(&config.dialect);
        Self {
            config: RwLock::new(config),
            config_path,
            engine,
            review: Mutex::new(ReviewState::new(Instant::now())),
            next_id: AtomicU64::new(1),
            candidates: Mutex::new(Vec::new()),
            keys: Mutex::new(HashMap::new()),
            paused: AtomicBool::new(false),
            watch_generation: AtomicU64::new(0),
            last_review: Default::default(),
            revision: AtomicU64::new(0),
        }
    }

    pub fn config(&self) -> Config {
        self.config.read().unwrap().clone()
    }

    pub fn next_session_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    pub fn key_for(&self, kind: ProviderKind, profile_id: &str) -> Option<String> {
        let account = crate::secrets::account(kind, profile_id);
        let mut keys = self.keys.lock().unwrap();
        keys.entry(account.clone()).or_insert_with(|| crate::secrets::get(&account)).clone()
    }

    pub fn set_cached_key(&self, kind: ProviderKind, profile_id: &str, key: Option<String>) {
        self.keys.lock().unwrap().insert(crate::secrets::account(kind, profile_id), key);
        self.last_review.forget();
    }

    /// Saves `config` as `save_config_based_on` does, and only while `profile`'s API key is still
    /// the one it carries. The keys are held for the save, so a key saved meanwhile either comes
    /// first and refuses this, or comes after it.
    pub fn save_config_for_checked(
        &self,
        config: Config,
        revision: u64,
        profile: &ProviderProfile,
    ) -> std::io::Result<Option<u64>> {
        let account = crate::secrets::account(profile.kind, &profile.id);
        let mut keys = self.keys.lock().unwrap();
        if *keys.entry(account.clone()).or_insert_with(|| crate::secrets::get(&account)) != profile.api_key {
            return Ok(None);
        }
        self.save_config_based_on(config, Some(revision))
    }

    /// The active provider with its API key attached, ready for a request.
    pub fn active_profile(&self) -> Option<ProviderProfile> {
        let mut profile = self.config.read().unwrap().active_profile()?.clone();
        profile.api_key = self.key_for(profile.kind, &profile.id);
        Some(profile)
    }

    pub fn profile(&self, id: &str) -> Option<ProviderProfile> {
        let mut profile = self.config.read().unwrap().providers.iter().find(|p| p.id == id)?.clone();
        profile.api_key = self.key_for(profile.kind, &profile.id);
        Some(profile)
    }

    /// The settings, and the revision they were saved at.
    pub fn config_at_revision(&self) -> (Config, u64) {
        let config = self.config.read().unwrap();
        (config.clone(), self.revision.load(Ordering::Relaxed))
    }

    pub fn save_config(&self, config: Config) -> std::io::Result<()> {
        self.save_config_based_on(config, None).map(|_| ())
    }

    /// Saves, unless `revision` is given and the settings have been saved since then. Returns the
    /// new revision, or None when the save was refused.
    pub fn save_config_based_on(&self, mut config: Config, revision: Option<u64>) -> std::io::Result<Option<u64>> {
        config.pin_official_addresses();
        // The revision changes only under this lock, so nothing can save between check and write.
        let mut current = self.config.write().unwrap();
        if revision.is_some_and(|r| r != self.revision.load(Ordering::Relaxed)) {
            return Ok(None);
        }
        config.save(&self.config_path)?;
        crate::platform::set_trace(config.diagnostic_log);
        *current = config;
        // Any setting may change what a review would say: the AI, its model, the styles, the word
        // list. The next review asks afresh.
        self.last_review.forget();
        Ok(Some(self.revision.fetch_add(1, Ordering::Relaxed) + 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_save_based_on_settings_read_before_another_save_is_refused() {
        let dir = std::env::temp_dir().join(format!("supereditr-revision-{}", std::process::id()));
        let state = AppState::new(dir.join("settings.json"));
        let (settings_copy, read_at) = state.config_at_revision();
        // The menu bar turns SuperEditr off in Notes while Settings has its copy.
        let mut menu = state.config();
        menu.set_excluded("com.apple.Notes", true);
        state.save_config(menu).unwrap();
        // Settings' save of its older copy is refused, so Notes stays excluded.
        let mut edited = settings_copy;
        edited.style_notes = "Short sentences.".into();
        assert_eq!(state.save_config_based_on(edited.clone(), Some(read_at)).unwrap(), None);
        assert!(state.config().is_excluded("com.apple.Notes"));
        assert!(Config::load(&dir.join("settings.json")).is_excluded("com.apple.Notes"));
        // Based on the current revision, the same save goes through.
        let (_, now) = state.config_at_revision();
        assert_eq!(state.save_config_based_on(edited, Some(now)).unwrap(), Some(now + 1));
        assert_eq!(state.config().style_notes, "Short sentences.");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn saving_the_settings_or_a_key_forgets_the_kept_reviews() {
        let dir = std::env::temp_dir().join(format!("supereditr-last-review-{}", std::process::id()));
        let state = AppState::new(dir.join("settings.json"));
        let now = Instant::now();
        // Any style's result kept for the selection: `last_style` finds none once all are gone.
        let shown_again = |state: &AppState| {
            let (_, captured, job) = crate::last_review::tests::example(now);
            state.last_review.last_style(&captured, now).is_some()
                || state.last_review.recall(&captured, &job, now).is_some()
        };
        state.last_review.remember(state.last_review.generation(), crate::last_review::tests::example(now).0);
        assert!(shown_again(&state));
        let mut config = state.config();
        config.words.push("Kyndal".into());
        state.save_config(config).unwrap();
        assert!(!shown_again(&state), "a settings save");

        state.last_review.remember(state.last_review.generation(), crate::last_review::tests::example(now).0);
        state.set_cached_key(ProviderKind::Anthropic, "anthropic", Some("sk-ant-0123456789".into()));
        assert!(!shown_again(&state), "a new API key");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Review number `id` of a selection in "Try it now", as a shortcut opens it.
    fn session(id: u64, text: &str) -> Session {
        let bounds = Rect { x: 10.0, y: 10.0, width: 300.0, height: 20.0 };
        Session {
            id,
            capture: crate::platform::playground_capture(text.into(), bounds),
            keyboard: true,
            anchor: Some(bounds),
            height: 210.0,
            task: None,
            run: 0,
            proofread: false,
            following: None,
        }
    }

    fn reviewing(id: u64) -> ReviewState {
        let mut review = ReviewState::new(Instant::now());
        assert!(review.begin(session(id, "Brynlee sent the KPI updaet.")).is_none());
        review
    }

    #[test]
    fn a_later_run_replaces_the_current_one_and_an_earlier_one_is_refused() {
        let mut review = reviewing(1);
        assert!(review.start_run(1, 0, true).is_some(), "the run the review starts with");
        assert!(review.session().unwrap().proofread);
        assert_eq!(review.start_run(1, 2, false).map(|s| s.run), Some(2));
        assert!(review.start_run(1, 1, true).is_none(), "run 1 was asked for before run 2");
        let current = review.session().unwrap();
        assert_eq!(current.run, 2);
        assert!(!current.proofread, "a refused run changes nothing");
        assert!(review.start_run(1, 2, true).is_some(), "the same run again, as R asks");
    }

    #[test]
    fn a_run_for_a_review_that_has_ended_starts_nothing() {
        let mut review = reviewing(2);
        assert!(review.start_run(1, 5, false).is_none());
        assert!(ReviewState::new(Instant::now()).start_run(2, 0, false).is_none());
    }

    #[test]
    fn a_second_review_replaces_the_first_and_hands_it_back() {
        let mut review = reviewing(1);
        let replaced = review.begin(session(2, "Kyndal wrote the OKR draft.")).unwrap();
        assert_eq!(replaced.id, 1);
        assert_eq!(review.session().unwrap().id, 2);
        assert!(review.current(1).is_none());
        assert!(review.is_showing_text("Kyndal wrote the OKR draft."));
        assert!(!review.is_showing_text("Brynlee sent the KPI updaet."));
    }

    #[test]
    fn a_notice_ends_the_review_and_a_new_review_hides_the_notice() {
        let mut review = reviewing(1);
        assert_eq!(review.show_notice().map(|s| s.id), Some(1));
        assert!(review.session().is_none());
        assert!(review.notice_visible() && review.showing());
        assert!(review.begin(session(2, "Rhyan sent the KPI report.")).is_none());
        assert!(!review.notice_visible());
        assert!(review.showing());
    }

    #[test]
    fn the_quiet_period_starts_with_a_fix_and_ends_after_quiet() {
        let start = Instant::now();
        let mut review = ReviewState::new(start);
        assert!(!review.is_quiet(start), "nothing was applied yet");
        review.start_quiet(start);
        assert!(review.is_quiet(start));
        assert!(review.is_quiet(start + QUIET - Duration::from_millis(1)));
        assert!(!review.is_quiet(start + QUIET));
    }

    #[test]
    fn typing_closes_the_panel_except_during_the_quiet_period() {
        let start = Instant::now();
        let mut review = ReviewState::new(start);
        assert!(!review.closes_on_interaction(start), "nothing is showing");
        review.show_notice();
        assert!(review.closes_on_interaction(start));
        review.start_quiet(start);
        assert!(!review.closes_on_interaction(start + Duration::from_millis(100)));
        assert!(review.closes_on_interaction(start + QUIET));
    }

    #[test]
    fn dismissing_a_review_that_is_no_longer_current_leaves_the_new_one() {
        let mut review = reviewing(1);
        review.begin(session(2, "Kyndal wrote the OKR draft."));
        assert!(matches!(review.dismiss(Some(1)), Dismissal::NotCurrent));
        assert_eq!(review.session().unwrap().id, 2);
        assert!(matches!(review.dismiss(Some(2)), Dismissal::Close(Some(ended)) if ended.id == 2));
        assert!(!review.showing());
    }

    #[test]
    fn dismissing_closes_a_notice_only_when_no_review_is_named() {
        let mut review = ReviewState::new(Instant::now());
        review.show_notice();
        assert!(matches!(review.dismiss(Some(3)), Dismissal::NotCurrent), "a notice has no review number");
        assert!(review.notice_visible());
        assert!(matches!(review.dismiss(None), Dismissal::Close(None)));
        assert!(!review.notice_visible());
        assert!(matches!(review.dismiss(None), Dismissal::Close(None)), "closing nothing still hides the panel");
    }

    #[test]
    fn applying_keeps_the_review_until_its_text_is_written() {
        let mut review = reviewing(1);
        review.start_run(1, 0, true);
        assert!(review.apply(2).is_none(), "another review");
        let applying = review.apply(1).unwrap();
        assert!(applying.proofread);
        assert_eq!(applying.capture.text, "Brynlee sent the KPI updaet.");
        assert_eq!(review.reviewed_pid(), Some(applying.capture.pid), "still showing while it's written");
        assert_eq!(review.end(1).map(|s| s.id), Some(1));
        assert!(review.apply(1).is_none(), "the review has ended");
    }

    #[test]
    fn a_review_that_begins_while_text_is_written_outlasts_the_earlier_one() {
        let mut review = reviewing(1);
        assert!(review.apply(1).is_some());
        // The user opens a new review while the first one's text is written into the app.
        assert_eq!(review.begin(session(2, "Kyndal wrote the OKR draft.")).map(|s| s.id), Some(1));
        assert!(review.end(1).is_none(), "the first review had already ended");
        assert_eq!(review.session().map(|s| s.id), Some(2), "the new review stays");
        assert_eq!(review.end(2).map(|s| s.id), Some(2));
        assert!(!review.showing());
    }

    #[test]
    fn a_review_that_couldnt_be_written_comes_back_with_the_keys() {
        let mut review = ReviewState::new(Instant::now());
        let mut automatic = session(1, "Brynlee sent the KPI updaet.");
        automatic.keyboard = false;
        review.begin(automatic);
        let mut following = None;
        assert!(!review.reopen(2, &mut following), "another review");
        assert!(!review.keyboard_review_open());
        assert!(review.reopen(1, &mut following));
        assert!(review.keyboard_review_open());
    }

    #[test]
    fn the_shortcut_takes_the_keys_only_from_an_automatic_review() {
        let mut review = ReviewState::new(Instant::now());
        assert_eq!(review.take_keyboard(), None, "nothing is showing");
        let mut automatic = session(4, "Kyndal wrote the OKR draft.");
        automatic.keyboard = false;
        review.begin(automatic);
        assert!(!review.keyboard_review_open());
        assert_eq!(review.take_keyboard(), Some(4));
        assert!(review.keyboard_review_open());
        assert_eq!(review.take_keyboard(), None, "it has the keys already: the shortcut starts a new review");
    }

    #[test]
    fn resizing_keeps_the_height_and_returns_where_the_selection_is() {
        let mut review = ReviewState::new(Instant::now());
        assert_eq!(review.resize(300.0), None);
        review.begin(session(1, "Rhyan sent the KPI report."));
        let anchor = review.session().unwrap().anchor;
        assert_eq!(review.resize(320.0), anchor);
        assert_eq!(review.session().unwrap().height, 320.0);
    }
}

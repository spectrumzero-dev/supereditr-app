//! Hyprland 0.55 and later, through its IPC sockets: `.socket.sock` takes one command per
//! connection, and `.socket2.sock` streams events.
//!
//! The panel's window rule and SuperEditr's binds are added at runtime with `eval`, never written to
//! the user's config. A config reload drops them, so they're kept here and added again on each
//! `configreloaded>>` event.
//!
//! A command connection is opened per request and closed at once (holding one open blocks
//! Hyprland), with a time limit on each, so a frozen compositor can't hold up SuperEditr. That makes
//! a request quick enough to make synchronously, even from the UI's code.

use super::lua::{self, OVERLAY_TITLE};
use super::{Capabilities, Desktop, DesktopError, SetupCheck, Shortcut, Subscription, WindowRef};
use crate::platform::Rect;
use async_trait::async_trait;
use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const REQUEST_LIMIT: Duration = Duration::from_millis(300);
/// The versions SuperEditr has been run on; the setup page flags any other.
const TESTED: &[(u32, u32)] = &[(0, 56), (0, 57)];
/// The oldest Hyprland with Lua dispatchers.
const OLDEST: (u32, u32) = (0, 55);

type FrontWatcher = Arc<dyn Fn(WindowRef) + Send + Sync>;

#[derive(Default)]
struct Runtime {
    /// The Lua for SuperEditr's binds, added again after a reload.
    binds: Vec<String>,
    /// The panel's latest rule.
    overlay: Option<String>,
}

pub struct HyprlandDesktop {
    commands: PathBuf,
    events: PathBuf,
    version: (u32, u32, u32),
    runtime: Mutex<Runtime>,
    watchers: Arc<Mutex<Vec<(u64, FrontWatcher)>>>,
}

impl HyprlandDesktop {
    /// Hyprland, when this session runs a version SuperEditr can work with.
    pub fn detect() -> Option<Self> {
        let signature = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok().filter(|s| !s.is_empty())?;
        let runtime = std::env::var("XDG_RUNTIME_DIR").ok().filter(|s| !s.is_empty())?;
        let dir = PathBuf::from(runtime).join("hypr").join(signature);
        let mut desktop = Self {
            commands: dir.join(".socket.sock"),
            events: dir.join(".socket2.sock"),
            version: (0, 0, 0),
            runtime: Mutex::default(),
            watchers: Arc::default(),
        };
        desktop.version = parse_version(&desktop.json("version").ok()?)?;
        ((desktop.version.0, desktop.version.1) >= OLDEST).then_some(desktop)
    }

    /// Sends one command and returns Hyprland's answer.
    fn request(&self, command: &str) -> Result<String, DesktopError> {
        let failed = |error: std::io::Error| match error.kind() {
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => DesktopError::Timeout,
            _ => DesktopError::Failed(format!("Hyprland’s socket: {error}")),
        };
        let mut socket = UnixStream::connect(&self.commands).map_err(failed)?;
        socket.set_read_timeout(Some(REQUEST_LIMIT)).map_err(failed)?;
        socket.set_write_timeout(Some(REQUEST_LIMIT)).map_err(failed)?;
        socket.write_all(command.as_bytes()).map_err(failed)?;
        let started = Instant::now();
        let mut answer = Vec::new();
        let mut buffer = [0; 16384];
        loop {
            match socket.read(&mut buffer).map_err(failed)? {
                0 => break,
                n => answer.extend_from_slice(&buffer[..n]),
            }
            if started.elapsed() > REQUEST_LIMIT {
                return Err(DesktopError::Timeout);
            }
        }
        Ok(String::from_utf8_lossy(&answer).into_owned())
    }

    fn json(&self, what: &str) -> Result<Value, DesktopError> {
        let answer = self.request(&format!("j/{what}"))?;
        serde_json::from_str(&answer).map_err(|_| DesktopError::Failed(format!("Hyprland’s {what}: {answer}")))
    }

    /// Runs a dispatcher, such as `hl.dsp.focus({ … })`.
    pub fn dispatch(&self, lua: &str) -> Result<(), DesktopError> {
        answer_ok(self.request(&format!("dispatch {lua}"))?)
    }

    /// Runs Lua in the compositor, such as `hl.bind(…)`.
    pub fn eval(&self, lua: &str) -> Result<(), DesktopError> {
        answer_ok(self.request(&format!("eval {lua}"))?)
    }

    pub fn windows(&self) -> Result<Vec<WindowRef>, DesktopError> {
        let clients = self.json("clients")?;
        Ok(clients.as_array().map(|all| all.iter().filter_map(parse_window).collect()).unwrap_or_default())
    }

    /// Hyprland's binds that aren't SuperEditr's, as (modifiers, key, what it does).
    pub fn other_binds(&self) -> Result<Vec<(u32, String, String)>, DesktopError> {
        Ok(parse_binds(&self.json("binds")?))
    }

    /// Replaces SuperEditr's binds: runs `remove` (unbinding the old keys) and `add`, and keeps `add`
    /// to run again after a config reload.
    pub fn set_binds(&self, remove: &[String], add: Vec<String>) -> Vec<String> {
        let mut problems = Vec::new();
        for lua in remove.iter().chain(&add) {
            if let Err(error) = self.eval(lua) {
                problems.push(error.to_string());
            }
        }
        self.runtime.lock().unwrap().binds = add;
        problems
    }

    /// Takes SuperEditr's binds and rule out of the session, when it quits.
    pub fn remove_all(&self, unbinds: &[String]) {
        for lua in unbinds {
            let _ = self.eval(lua);
        }
        let _ = self.eval(&lua::remove_overlay_rule());
        *self.runtime.lock().unwrap() = Runtime::default();
    }

    /// Follows Hyprland's events for as long as SuperEditr runs: the front window changing, and
    /// config reloads, which drop SuperEditr's binds and rule. Reconnects if Hyprland restarts.
    pub fn follow_events(self: &Arc<Self>) {
        let desktop = self.clone();
        std::thread::spawn(move || {
            loop {
                if let Ok(socket) = UnixStream::connect(&desktop.events) {
                    for line in BufReader::new(socket).lines() {
                        let Ok(line) = line else { break };
                        desktop.on_event(&line);
                    }
                }
                std::thread::sleep(Duration::from_secs(2));
            }
        });
    }

    fn on_event(&self, line: &str) {
        let Some((event, _)) = line.split_once(">>") else { return };
        match event {
            "configreloaded" => {
                let (binds, overlay) = {
                    let runtime = self.runtime.lock().unwrap();
                    (runtime.binds.clone(), runtime.overlay.clone())
                };
                for lua in binds.iter().chain(&overlay) {
                    let _ = self.eval(lua);
                }
                super::super::trace(format_args!(
                    "Hyprland reloaded its config; added {} binds and the panel rule again",
                    binds.len()
                ));
            }
            "activewindowv2" => {
                let watchers: Vec<FrontWatcher> =
                    self.watchers.lock().unwrap().iter().map(|(_, watcher)| watcher.clone()).collect();
                if watchers.is_empty() {
                    return;
                }
                if let Ok(window) = self.front_window() {
                    for watcher in watchers {
                        watcher(window.clone());
                    }
                }
            }
            _ => {}
        }
    }

    /// Whether SuperEditr's panel is the focused window.
    pub fn overlay_focused(&self) -> bool {
        self.front_window().is_ok_and(|window| window.pid == std::process::id() as i32 && window.title == OVERLAY_TITLE)
    }
}

/// Hyprland answers "ok", or an error or warning ("warning: … window not found").
fn answer_ok(answer: String) -> Result<(), DesktopError> {
    if answer.trim() == "ok" { Ok(()) } else { Err(DesktopError::Failed(answer.trim().to_string())) }
}

fn parse_version(version: &Value) -> Option<(u32, u32, u32)> {
    let text = version["version"].as_str().or_else(|| version["tag"].as_str())?;
    let mut parts = text.trim_start_matches('v').split('.').map(|part| part.parse::<u32>().ok());
    Some((parts.next()??, parts.next()??, parts.next().flatten().unwrap_or(0)))
}

/// The top-left corner of the focused monitor, from `monitors`.
fn focused_origin(monitors: &Value) -> Option<(f64, f64)> {
    let focused = monitors.as_array()?.iter().find(|m| m["focused"].as_bool() == Some(true))?;
    Some((focused["x"].as_f64()?, focused["y"].as_f64()?))
}

/// A window from `activewindow` or `clients`; `None` for an empty answer (no window in front).
fn parse_window(window: &Value) -> Option<WindowRef> {
    let handle = window["address"].as_str().filter(|a| !a.is_empty())?.to_string();
    let pair = |key: &str| Some((window[key][0].as_f64()?, window[key][1].as_f64()?));
    let rect = pair("at").zip(pair("size")).map(|((x, y), (width, height))| Rect { x, y, width, height });
    Some(WindowRef {
        handle,
        pid: window["pid"].as_i64().unwrap_or(-1) as i32,
        app_id: window["class"].as_str().unwrap_or_default().to_string(),
        title: window["title"].as_str().unwrap_or_default().to_string(),
        rect,
        xwayland: window["xwayland"].as_bool().unwrap_or(false),
        tags: window["tags"]
            .as_array()
            .map(|tags| tags.iter().filter_map(|t| t.as_str().map(String::from)).collect())
            .unwrap_or_default(),
    })
}

fn parse_binds(binds: &Value) -> Vec<(u32, String, String)> {
    binds
        .as_array()
        .map(|all| {
            all.iter()
                .filter(|bind| !bind["description"].as_str().unwrap_or_default().starts_with("SuperEditr"))
                .filter(|bind| bind["submap"].as_str().unwrap_or_default().is_empty())
                .filter_map(|bind| {
                    let what = bind["description"]
                        .as_str()
                        .filter(|d| !d.is_empty())
                        .or_else(|| bind["dispatcher"].as_str())
                        .unwrap_or_default();
                    Some((bind["modmask"].as_u64()? as u32, bind["key"].as_str()?.to_string(), what.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Hyprland's modifier mask for SuperEditr's modifiers.
pub fn modmask(modifiers: u8) -> u32 {
    use crate::double_press::{ALT, COMMAND, CONTROL, SHIFT};
    [(SHIFT, 1), (CONTROL, 4), (ALT, 8), (COMMAND, 64)]
        .iter()
        .filter(|(bit, _)| modifiers & bit != 0)
        .map(|(_, mask)| mask)
        .sum()
}

#[async_trait]
impl Desktop for HyprlandDesktop {
    fn name(&self) -> &'static str {
        "Hyprland"
    }

    async fn capabilities(&self, window: &WindowRef) -> Capabilities {
        let known = !window.handle.is_empty();
        Capabilities { send_keys: known, focus: known }
    }

    fn front_window(&self) -> Result<WindowRef, DesktopError> {
        parse_window(&self.json("activewindow")?).ok_or_else(|| DesktopError::Failed("no window is in front".into()))
    }

    fn watch_front_window(&self, on_change: Box<dyn Fn(WindowRef) + Send + Sync>) -> Subscription {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        self.watchers.lock().unwrap().push((id, Arc::from(on_change)));
        let watchers = self.watchers.clone();
        Subscription::new(move || watchers.lock().unwrap().retain(|(watcher, _)| *watcher != id))
    }

    fn cursor(&self) -> Result<(f64, f64), DesktopError> {
        let at = self.json("cursorpos")?;
        Ok((at["x"].as_f64().unwrap_or(0.0), at["y"].as_f64().unwrap_or(0.0)))
    }

    fn place_overlay(&self, at: Rect, shown: bool) -> Result<(), DesktopError> {
        // A rule's position counts from the corner of the monitor the window opens on, the focused
        // one; a move's counts from the desktop's.
        let (x, y) = self.json("monitors").ok().and_then(|all| focused_origin(&all)).unwrap_or((0.0, 0.0));
        let rule = lua::overlay_rule(Rect { x: at.x - x, y: at.y - y, ..at });
        self.eval(&rule)?;
        self.runtime.lock().unwrap().overlay = Some(rule);
        // GTK calls the panel shown before Hyprland maps it. A move sent in between is lost, and the
        // rule above places the panel instead, so the rule is kept up to date as well.
        if shown {
            self.dispatch(&lua::move_overlay(at.x.round() as i32, at.y.round() as i32))?;
        }
        Ok(())
    }

    async fn focus(&self, window: &WindowRef) -> Result<(), DesktopError> {
        self.dispatch(&lua::focus(&lua::window(window)))
    }

    async fn focus_overlay(&self) -> Result<(), DesktopError> {
        // The panel may not be mapped yet just after it's shown: try until it has focus.
        let started = Instant::now();
        loop {
            let _ = self.dispatch(&lua::focus_overlay());
            if self.overlay_focused() {
                return Ok(());
            }
            if started.elapsed() > Duration::from_millis(500) {
                return Err(DesktopError::Timeout);
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    }

    async fn send_shortcut(&self, window: &WindowRef, keys: Shortcut) -> Result<(), DesktopError> {
        self.dispatch(&lua::send_shortcut(&keys, &lua::window(window)))
    }

    fn setup_status(&self) -> Vec<SetupCheck> {
        let (major, minor, patch) = self.version;
        let tested = TESTED.contains(&(major, minor));
        vec![SetupCheck {
            name: "Hyprland".into(),
            ok: tested,
            detail: if tested {
                format!("{major}.{minor}.{patch}")
            } else {
                format!("{major}.{minor}.{patch}, which SuperEditr hasn’t been tested on")
            },
        }]
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// `hyprctl -j activewindow` on Omarchy 4.0.6's Hyprland 0.56.2, with SuperEditr's Settings in
    /// front.
    const ACTIVE_0_56: &str = r#"{
    "address": "0x562229d30760", "mapped": true, "hidden": false, "visible": true, "acceptsInput": true,
    "at": [12, 38], "size": [1256, 750], "workspace": { "id": 1, "name": "1" }, "floating": false,
    "monitor": 0, "class": "supereditr", "title": "SuperEditr Settings", "initialClass": "supereditr",
    "initialTitle": "SuperEditr Settings", "pid": 11592, "xwayland": false, "pinned": false,
    "pinFullscreened": false, "fullscreen": 0, "fullscreenClient": 0, "fullscreenHandler": "default",
    "allowedOverFullscreen": true, "grouped": [], "tags": ["default-opacity*"], "swallowing": "0x0",
    "focusHistoryID": 0, "inhibitingIdle": false, "xdgTag": "", "xdgDescription": "", "contentType": "none",
    "tearingHint": false, "stableId": "18000008"
}"#;

    /// The same window in 0.57.0's format (`src/ipc/s1/Commands.cpp`): the workspace gains an
    /// address and a type.
    const ACTIVE_0_57: &str = r#"{
    "address": "0x55d0c1a2b3c0", "mapped": true, "hidden": false, "visible": true, "acceptsInput": true,
    "at": [640, 38], "size": [628, 750],
    "workspace": { "address": "0x55d0c19f0a10", "id": 1, "type": "normal", "name": "1" },
    "floating": false, "monitor": 0, "class": "foot", "title": "rhyan@quillpoint: ~", "initialClass": "foot",
    "initialTitle": "foot", "pid": 2817, "xwayland": false, "pinned": false, "pinFullscreened": false,
    "fullscreen": 0, "fullscreenClient": 0, "fullscreenHandler": "default", "allowedOverFullscreen": true,
    "grouped": [], "tags": ["terminal*", "default-opacity*"], "swallowing": "0x0", "focusHistoryID": 0,
    "inhibitingIdle": false, "xdgTag": "", "xdgDescription": "", "contentType": "none", "tearingHint": false,
    "stableId": "1a"
}"#;

    #[test]
    fn windows_parse_from_both_versions() {
        let window = parse_window(&serde_json::from_str(ACTIVE_0_56).unwrap()).unwrap();
        assert_eq!(window.handle, "0x562229d30760");
        assert_eq!((window.pid, window.app_id.as_str(), window.xwayland), (11592, "supereditr", false));
        assert_eq!(window.rect, Some(Rect { x: 12.0, y: 38.0, width: 1256.0, height: 750.0 }));

        let foot = parse_window(&serde_json::from_str(ACTIVE_0_57).unwrap()).unwrap();
        assert_eq!(foot.app_id, "foot");
        assert_eq!(foot.tags, ["terminal*", "default-opacity*"]);
        assert!(crate::platform::linux::apps::profile(&foot.app_id, &foot.tags).terminal);
    }

    #[test]
    fn no_window_in_front_is_an_empty_answer() {
        assert_eq!(parse_window(&serde_json::from_str("{}").unwrap()), None);
    }

    #[test]
    fn versions_parse_and_old_ones_are_refused() {
        let recorded = r#"{"branch": "v0.56.2", "commit": "efb50993780079460b0cbed1363e2166a2de1d9f", "version": "0.56.2",
            "dirty": false, "tag": "v0.56.2", "commits": "7661", "flags": []}"#;
        assert_eq!(parse_version(&serde_json::from_str(recorded).unwrap()), Some((0, 56, 2)));
        assert_eq!(parse_version(&serde_json::from_str(r#"{"tag": "v0.57.0"}"#).unwrap()), Some((0, 57, 0)));
        assert!((0, 54) < OLDEST && (0, 56) >= OLDEST);
    }

    #[test]
    fn binds_that_arent_supereditrs_are_listed_with_their_masks() {
        // From `hyprctl -j binds` on Omarchy 4.0.6: SUPER+CTRL+E, SUPER+SHIFT+E and SUPER+ALT+SHIFT+E.
        let binds = r#"[
            {"modmask": 68, "submap": "", "key": "E", "description": "Emojis", "dispatcher": "__lua", "arg": "21"},
            {"modmask": 65, "submap": "", "key": "E", "description": "Email", "dispatcher": "__lua", "arg": "306"},
            {"modmask": 73, "submap": "", "key": "E", "description": "New email", "dispatcher": "__lua", "arg": "308"},
            {"modmask": 72, "submap": "", "key": "E", "description": "SuperEditr: review the selection", "dispatcher": "__lua", "arg": "400"},
            {"modmask": 0, "submap": "capture", "key": "RETURN", "description": "Capture highlighted window", "dispatcher": "__lua", "arg": "9"}
        ]"#;
        let others = parse_binds(&serde_json::from_str(binds).unwrap());
        assert_eq!(
            others,
            [(68, "E".into(), "Emojis".into()), (65, "E".into(), "Email".into()), (73, "E".into(), "New email".into())]
        );
        use crate::double_press::{ALT, COMMAND, CONTROL, SHIFT};
        assert_eq!(modmask(COMMAND | CONTROL), 68);
        assert_eq!(modmask(COMMAND | ALT | SHIFT), 73);
        assert_eq!(modmask(COMMAND | ALT), 72);
    }

    /// `hyprctl -j monitors` on Hyprland 0.56.2 with a second monitor added and focused, trimmed.
    const MONITORS: &str = r#"[
        {"id": 0, "name": "Virtual-1", "width": 1280, "height": 800, "x": 2240, "y": 0, "scale": 1, "focused": false},
        {"id": 1, "name": "HEADLESS-2", "width": 1600, "height": 900, "x": 1280, "y": 0, "scale": 1.6666666, "focused": true}
    ]"#;

    /// A Hyprland that records each command and answers it, at a socket in a new directory.
    pub(crate) fn fake_hyprland(answer: fn(&str) -> String) -> (HyprlandDesktop, Arc<Mutex<Vec<String>>>) {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "supereditr-hyprland-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let listener = std::os::unix::net::UnixListener::bind(dir.join(".socket.sock")).unwrap();
        let commands = Arc::new(Mutex::new(Vec::new()));
        let seen = commands.clone();
        std::thread::spawn(move || {
            for mut socket in listener.incoming().map_while(Result::ok) {
                let mut buffer = [0; 16384];
                let n = socket.read(&mut buffer).unwrap_or(0);
                let command = String::from_utf8_lossy(&buffer[..n]).into_owned();
                let _ = socket.write_all(answer(&command).as_bytes());
                seen.lock().unwrap().push(command);
            }
        });
        let desktop = HyprlandDesktop {
            commands: dir.join(".socket.sock"),
            events: dir.join(".socket2.sock"),
            version: (0, 56, 2),
            runtime: Mutex::default(),
            watchers: Arc::default(),
        };
        (desktop, commands)
    }

    fn answer_with_monitors(command: &str) -> String {
        if command == "j/monitors" { MONITORS.into() } else { "ok".into() }
    }

    #[test]
    fn the_focused_monitor_is_found() {
        assert_eq!(focused_origin(&serde_json::from_str(MONITORS).unwrap()), Some((1280.0, 0.0)));
        assert_eq!(focused_origin(&serde_json::from_str("[]").unwrap()), None);
    }

    #[test]
    fn the_panels_rule_counts_from_the_focused_monitor() {
        let (desktop, commands) = fake_hyprland(answer_with_monitors);
        let at = Rect { x: 1278.0, y: 108.0, width: 548.0, height: 210.0 };
        desktop.place_overlay(at, false).unwrap();
        let rule = lua::overlay_rule(Rect { x: -2.0, ..at });
        assert_eq!(*commands.lock().unwrap(), ["j/monitors".to_string(), format!("eval {rule}")]);
        assert_eq!(desktop.runtime.lock().unwrap().overlay, Some(rule));
    }

    #[test]
    fn a_shown_panel_moves_and_its_rule_follows() {
        // GTK calls the panel shown before Hyprland maps it, and the rule places it at the map.
        let (desktop, commands) = fake_hyprland(answer_with_monitors);
        let at = Rect { x: 1290.0, y: 297.0, width: 548.0, height: 531.0 };
        desktop.place_overlay(at, true).unwrap();
        assert_eq!(
            *commands.lock().unwrap(),
            [
                "j/monitors".to_string(),
                format!("eval {}", lua::overlay_rule(Rect { x: 10.0, ..at })),
                format!("dispatch {}", lua::move_overlay(1290, 297)),
            ]
        );
    }

    #[test]
    fn without_a_focused_monitor_the_rule_counts_from_the_desktops_corner() {
        let (desktop, commands) = fake_hyprland(|_| "ok".into());
        let at = Rect { x: 590.0, y: 415.0, width: 548.0, height: 210.0 };
        desktop.place_overlay(at, false).unwrap();
        assert_eq!(commands.lock().unwrap()[1], format!("eval {}", lua::overlay_rule(at)));
    }

    #[test]
    fn hyprlands_answer_is_ok_or_a_reason() {
        assert!(answer_ok("ok".into()).is_ok());
        let error = answer_ok("warning: =[C]:-1: hl.focus: window not found".into()).unwrap_err();
        assert_eq!(error.to_string(), "warning: =[C]:-1: hl.focus: window not found");
    }
}

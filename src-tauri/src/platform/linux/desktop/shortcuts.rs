//! Global shortcuts on Hyprland. Wayland gives no app the keyboard, so the compositor owns the keys.
//!
//! The main path: SuperEditr registers its app ID with the portals (which needs its
//! `.desktop` file), creates a GlobalShortcuts session and binds `review` and `quick-fix`. Hyprland's
//! portal doesn't take keys from apps, so SuperEditr adds its own binds at runtime, each sending
//! `hl.dsp.global("app.supereditr:review")`, and the portal's `Activated` signal reaches the running
//! app. Where the portal can't be used, the binds run `supereditr review` instead, which the
//! single-instance plugin hands to the running app.

use super::hyprland::{HyprlandDesktop, modmask};
use super::lua::{self, BindAction, PORTAL_APP_ID};
use super::{Action, Binding, ShortcutBackend, Subscription};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    /// The portal session isn't set up yet: binds wait for it.
    Pending,
    Portal,
    Exec,
}

type OnFire = Arc<dyn Fn(Action) + Send + Sync>;

struct State {
    route: Route,
    wanted: Vec<Binding>,
    /// Bound now, to unbind when they change.
    bound: Vec<Binding>,
    on_fire: Option<OnFire>,
}

pub struct HyprlandShortcuts {
    hyprland: Arc<HyprlandDesktop>,
    state: Mutex<State>,
}

impl HyprlandShortcuts {
    pub fn new(hyprland: Arc<HyprlandDesktop>) -> Arc<Self> {
        Arc::new(Self {
            hyprland,
            state: Mutex::new(State { route: Route::Pending, wanted: Vec::new(), bound: Vec::new(), on_fire: None }),
        })
    }

    /// Sets up the portal session, then binds the keys asked for meanwhile. Runs for as long as
    /// SuperEditr does, passing on each activation.
    pub async fn start(self: Arc<Self>) {
        match tokio::time::timeout(Duration::from_secs(5), portal_session()).await {
            Ok(Ok((portal, _session))) => {
                use futures_util::StreamExt;
                let Ok(activations) = portal.receive_activated().await else { return self.use_route(Route::Exec) };
                self.use_route(Route::Portal);
                let mut activations = std::pin::pin!(activations);
                while let Some(activated) = activations.next().await {
                    let on_fire = self.state.lock().unwrap().on_fire.clone();
                    if let (Some(action), Some(on_fire)) = (Action::from_id(activated.shortcut_id()), on_fire) {
                        crate::platform::trace(format_args!("shortcut {} from the portal", action.id()));
                        on_fire(action);
                    }
                }
            }
            Ok(Err(error)) => {
                crate::platform::trace(format_args!("GlobalShortcuts portal unavailable ({error}); binding commands"));
                self.use_route(Route::Exec);
            }
            Err(_) => {
                crate::platform::trace(format_args!("GlobalShortcuts portal didn’t answer; binding commands"));
                self.use_route(Route::Exec);
            }
        }
    }

    fn use_route(&self, route: Route) {
        let wanted = {
            let mut state = self.state.lock().unwrap();
            state.route = route;
            state.wanted.clone()
        };
        let problems = self.apply(&wanted);
        if !problems.is_empty() {
            eprintln!("SuperEditr: some shortcuts could not be bound: {problems:?}");
        }
    }

    /// What already uses this key in Hyprland, other than SuperEditr.
    pub fn taken_by(&self, binding: &Binding) -> Option<String> {
        let mask = modmask(binding.shortcut.modifiers);
        self.hyprland
            .other_binds()
            .ok()?
            .into_iter()
            .find(|(m, key, _)| *m == mask && key.eq_ignore_ascii_case(&binding.shortcut.key))
            .map(|(_, _, what)| what)
    }

    fn apply(&self, wanted: &[Binding]) -> Vec<String> {
        let mut state = self.state.lock().unwrap();
        let route = state.route;
        if route == Route::Pending {
            return Vec::new();
        }
        let remove = unbinds(&state.bound, wanted);
        let command = command();
        let mut problems = Vec::new();
        let mut bound = Vec::new();
        let mut add = Vec::new();
        for binding in wanted {
            // A key Omarchy (or the user) already uses would do both things.
            if let Some(what) = self.taken_by(binding) {
                problems.push(format!("{} is already used for “{what}”", binding.shortcut.label()));
                continue;
            }
            let exec = format!("{command} {}", binding.action.id());
            let action = match route {
                Route::Portal => BindAction::Portal(binding.action.id()),
                _ => BindAction::Exec(&exec),
            };
            add.push(lua::bind(&binding.shortcut, action, binding.action.description()));
            bound.push(binding.clone());
        }
        problems.extend(self.hyprland.set_binds(&remove, add));
        state.bound = bound;
        problems
    }
}

/// What to unbind before binding `wanted`: the keys bound before, and the ones about to be. A
/// SuperEditr that was killed leaves its binds behind, and a key bound twice runs twice.
fn unbinds(bound: &[Binding], wanted: &[Binding]) -> Vec<String> {
    let mut remove: Vec<String> = Vec::new();
    for binding in bound.iter().chain(wanted) {
        let unbind = lua::unbind(&binding.shortcut);
        if !remove.contains(&unbind) {
            remove.push(unbind);
        }
    }
    remove
}

/// How a bind starts SuperEditr: its installed path, where it has one.
fn command() -> String {
    std::env::current_exe()
        .ok()
        .map(|exe| exe.display().to_string())
        .filter(|exe| exe.starts_with("/usr/") && !exe.contains(char::is_whitespace))
        .unwrap_or_else(|| "supereditr".into())
}

async fn portal_session() -> Result<
    (
        ashpd::desktop::global_shortcuts::GlobalShortcuts,
        ashpd::desktop::Session<ashpd::desktop::global_shortcuts::GlobalShortcuts>,
    ),
    ashpd::Error,
> {
    use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};
    // The registration ties this connection to the app ID, so the session must use the same one.
    let connection = zbus::Connection::session().await?;
    let app_id = ashpd::AppID::try_from(PORTAL_APP_ID).map_err(|_| ashpd::Error::InvalidAppID)?;
    ashpd::register_host_app_with_connection(connection.clone(), app_id).await?;
    let portal = GlobalShortcuts::with_connection(connection).await?;
    let session = portal.create_session(Default::default()).await?;
    let shortcuts: Vec<NewShortcut> = [Action::Review, Action::QuickFix]
        .into_iter()
        .map(|action| NewShortcut::new(action.id(), format!("SuperEditr: {}", action.description())))
        .collect();
    portal.bind_shortcuts(&session, &shortcuts, None, Default::default()).await?.response()?;
    Ok((portal, session))
}

impl ShortcutBackend for HyprlandShortcuts {
    fn install(&self, bindings: &[Binding]) -> Vec<String> {
        self.state.lock().unwrap().wanted = bindings.to_vec();
        self.apply(bindings)
    }

    fn uninstall(&self) {
        let mut state = self.state.lock().unwrap();
        let unbinds: Vec<String> = state.bound.drain(..).map(|b| lua::unbind(&b.shortcut)).collect();
        self.hyprland.remove_all(&unbinds);
    }

    fn listen(&self, on_fire: Box<dyn Fn(Action) + Send + Sync>) -> Subscription {
        self.state.lock().unwrap().on_fire = Some(Arc::from(on_fire));
        Subscription::none()
    }
}

#[cfg(test)]
mod tests {
    use super::super::Shortcut;
    use super::*;

    fn binding(action: Action, accelerator: &str) -> Binding {
        Binding { action, shortcut: Shortcut::parse(accelerator).unwrap() }
    }

    #[test]
    fn binding_first_unbinds_the_old_keys_and_the_new_ones_once() {
        let before = [binding(Action::Review, "Super+Alt+E"), binding(Action::QuickFix, "Super+Control+Alt+E")];
        let after = [binding(Action::Review, "Super+Alt+R"), binding(Action::QuickFix, "Super+Control+Alt+E")];
        assert_eq!(
            unbinds(&before, &after),
            [
                r#"hl.unbind("SUPER + ALT + E")"#,
                r#"hl.unbind("SUPER + CTRL + ALT + E")"#,
                r#"hl.unbind("SUPER + ALT + R")"#,
            ]
        );
        // After a restart nothing is bound yet, but a killed SuperEditr's binds may still be there.
        assert_eq!(unbinds(&[], &before).len(), 2);
    }
}

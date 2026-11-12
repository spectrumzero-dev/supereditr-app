//! Opening at login: a Run value for the current user, with the program's path in quotes.

use windows_registry::{CURRENT_USER, Type};

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// Where Task Manager and Settings › Apps › Startup record that the user turned a Run value off.
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
/// The Run value's name, the same one earlier builds wrote through the autostart plugin. The
/// installer's uninstaller removes it.
const NAME: &str = "supereditr";
/// What `APPROVED` holds for a value that's on.
const APPROVED_ON: [u8; 12] = [2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

/// The Run value for `exe`. Windows splits an unquoted command at its first space, so a path with a
/// space in it, such as a user name with one, would start a program that isn't there.
fn command(exe: &str) -> String {
    format!("\"{exe}\"")
}

/// Whether an `APPROVED` value leaves the Run value on: Windows writes the time it was turned off
/// into the last eight bytes, and zeros while it's on.
fn approved(value: &[u8]) -> bool {
    value.len() < 8 || value[value.len() - 8..].iter().all(|&b| b == 0)
}

pub fn is_enabled() -> Result<bool, String> {
    let registered = CURRENT_USER.open(RUN).and_then(|key| key.get_string(NAME)).is_ok();
    let on = CURRENT_USER.open(APPROVED).and_then(|key| key.get_value(NAME)).map_or(true, |value| approved(&value));
    Ok(registered && on)
}

/// Turns opening at login on, for this copy of the app. Turning it on here also undoes the user
/// turning it off in Task Manager, as the switch in Settings says it's on.
pub fn enable() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    CURRENT_USER
        .create(RUN)
        .and_then(|key| key.set_string(NAME, command(&exe.display().to_string())))
        .map_err(|e| e.to_string())?;
    if let Ok(key) = CURRENT_USER.options().read().write().open(APPROVED)
        && key.get_value(NAME).is_ok_and(|value| !approved(&value))
    {
        key.set_bytes(NAME, Type::Bytes, &APPROVED_ON).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn disable() -> Result<(), String> {
    for key in [RUN, APPROVED] {
        if let Ok(key) = CURRENT_USER.options().write().open(key) {
            let _ = key.remove_value(NAME);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_run_value_quotes_the_path() {
        assert_eq!(
            command(r"C:\Users\Rhyan Brynlee\AppData\Local\SuperEditr\supereditr.exe"),
            r#""C:\Users\Rhyan Brynlee\AppData\Local\SuperEditr\supereditr.exe""#
        );
    }

    #[test]
    fn task_manager_marks_a_value_off_with_the_time() {
        assert!(approved(&APPROVED_ON));
        assert!(approved(&[6, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        assert!(!approved(&[3, 0, 0, 0, 0x50, 0x6b, 0x1d, 0x8a, 0xd2, 0x51, 0xdb, 0x01]));
        assert!(approved(&[]), "too short to say it's off");
    }
}

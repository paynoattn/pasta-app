//! The global shortcut for the clipboard panel (#38, #48).
//!
//! Only macOS has a panel so far; on Windows and KDE the Shortcuts page shows
//! a placeholder for the future history panel. A shortcut must have a
//! modifier (function keys may stand alone), must parse as an accelerator,
//! and is checked against shortcuts the OS or most apps already use.

use std::sync::Mutex;

use tauri::AppHandle;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

use crate::views::ShortcutsView;

/// ⇧⌘V, as in Paste.app.
pub const DEFAULT_PANEL: &str = "Shift+Command+V";

/// Why the current shortcut couldn't be registered, for the Shortcuts page.
static LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);

/// Shortcuts that belong to the OS: registering them would break it, or never fire.
const RESERVED: &[(&str, &str)] = &[
    ("Command+Q", "quits the frontmost app"),
    ("Command+W", "closes windows"),
    ("Command+Tab", "switches apps"),
    ("Command+Space", "opens Spotlight"),
    ("Control+Space", "switches input sources"),
    ("Command+C", "copies"),
    ("Command+V", "pastes"),
    ("Command+X", "cuts"),
    ("Command+Z", "undoes"),
    ("Command+A", "selects all"),
    ("Shift+Command+3", "takes a screenshot"),
    ("Shift+Command+4", "takes a screenshot"),
    ("Shift+Command+5", "opens Screenshot"),
    ("Control+Command+Q", "locks the screen"),
    ("Alt+Command+Escape", "opens Force Quit"),
];

/// Shortcuts that work but take over something other apps use.
const KNOWN_CLASHES: &[(&str, &str)] = &[
    (
        "Shift+Command+V",
        "Many apps (Notes, Mail, Pages, Slack) use ⇧⌘V for Paste and Match Style. Pasta takes it over while it runs; \
         use ⇧↩ in the panel to paste as plain text instead.",
    ),
    ("Alt+Command+V", "Finder uses ⌥⌘V to move files you copied."),
    ("Control+Command+Space", "macOS opens the emoji picker with ⌃⌘Space."),
    ("Shift+Command+A", "Some browsers use ⇧⌘A to search tabs."),
];

/// Puts a shortcut in Pasta's canonical form, e.g. `cmd+shift+v` → `Shift+Command+V`.
pub fn normalize(accelerator: &str) -> Result<String, String> {
    let mut modifiers = [false; 4]; // Control, Alt, Shift, Command
    let mut key: Option<String> = None;
    for part in accelerator.split('+').map(str::trim).filter(|part| !part.is_empty()) {
        match part.to_ascii_lowercase().as_str() {
            "control" | "ctrl" => modifiers[0] = true,
            "alt" | "option" => modifiers[1] = true,
            "shift" => modifiers[2] = true,
            "command" | "cmd" | "super" | "meta" => modifiers[3] = true,
            other => {
                if key.is_some() {
                    return Err("a shortcut has one key besides the modifiers".into());
                }
                key = Some(normalize_key(other).ok_or_else(|| format!("“{part}” isn't a key Pasta can use"))?);
            }
        }
    }
    let key = key.ok_or("press a key together with the modifiers")?;
    let function_key = key.len() > 1 && key.starts_with('F') && key[1..].chars().all(|c| c.is_ascii_digit());
    if !modifiers.iter().any(|on| *on) && !function_key {
        return Err("add ⌘, ⌃, ⌥ or ⇧ so the shortcut doesn't fire while you type".into());
    }
    let names = ["Control", "Alt", "Shift", "Command"];
    let mut parts: Vec<&str> = names.iter().zip(modifiers).filter(|(_, on)| *on).map(|(name, _)| *name).collect();
    parts.push(&key);
    Ok(parts.join("+"))
}

fn normalize_key(key: &str) -> Option<String> {
    let key = key.strip_prefix("key").filter(|rest| rest.len() == 1).unwrap_or(key);
    let key = key.strip_prefix("digit").filter(|rest| rest.len() == 1).unwrap_or(key);
    if key.len() == 1 && key.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Some(key.to_ascii_uppercase());
    }
    if let Some(number) = key.strip_prefix('f')
        && number.parse::<u8>().is_ok_and(|n| (1..=20).contains(&n))
    {
        return Some(format!("F{number}"));
    }
    let named = match key {
        "space" => "Space",
        "enter" | "return" => "Enter",
        "tab" => "Tab",
        "escape" | "esc" => "Escape",
        "backspace" => "Backspace",
        "delete" => "Delete",
        "up" | "arrowup" => "Up",
        "down" | "arrowdown" => "Down",
        "left" | "arrowleft" => "Left",
        "right" | "arrowright" => "Right",
        "comma" => "Comma",
        "period" => "Period",
        "slash" => "Slash",
        "semicolon" => "Semicolon",
        "quote" => "Quote",
        "backquote" => "Backquote",
        "minus" => "Minus",
        "equal" => "Equal",
        "bracketleft" => "BracketLeft",
        "bracketright" => "BracketRight",
        "backslash" => "Backslash",
        _ => return None,
    };
    Some(named.to_owned())
}

/// Refuses shortcuts the OS owns.
pub fn check_reserved(normalized: &str) -> Result<(), String> {
    match RESERVED.iter().find(|(shortcut, _)| *shortcut == normalized) {
        Some((_, what)) => Err(format!("{} {what}; pick another shortcut", display(normalized))),
        None => Ok(()),
    }
}

/// A heads-up about apps that use the same shortcut.
pub fn clash(normalized: &str) -> Option<String> {
    KNOWN_CLASHES.iter().find(|(shortcut, _)| *shortcut == normalized).map(|(_, note)| (*note).to_owned())
}

/// `Shift+Command+V` → `⇧⌘V`.
pub fn display(normalized: &str) -> String {
    normalized
        .split('+')
        .map(|part| match part {
            "Control" => "⌃".to_owned(),
            "Alt" => "⌥".to_owned(),
            "Shift" => "⇧".to_owned(),
            "Command" => "⌘".to_owned(),
            "Space" => "Space".to_owned(),
            other => other.to_owned(),
        })
        .collect()
}

pub fn supported() -> bool {
    cfg!(target_os = "macos")
}

/// Registers `accelerator` as the panel shortcut, replacing the previous one.
pub fn register_panel(app: &AppHandle, accelerator: &str) -> Result<(), String> {
    let normalized = normalize(accelerator)?;
    check_reserved(&normalized)?;
    let shortcut: Shortcut =
        normalized.parse().map_err(|err| format!("{} can't be used: {err}", display(&normalized)))?;
    let manager = app.global_shortcut();
    let _ = manager.unregister_all();
    let result = manager.on_shortcut(shortcut, |app, _shortcut, event| {
        if event.state == ShortcutState::Pressed {
            crate::ui::toggle_panel(app);
        }
    });
    let outcome = result
        .map_err(|err| format!("{} couldn't be registered; another app may be using it ({err})", display(&normalized)));
    *LAST_ERROR.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = outcome.as_ref().err().cloned();
    match &outcome {
        Ok(()) => tracing::info!(shortcut = %normalized, "registered the panel shortcut"),
        Err(err) => tracing::warn!(error = %err, "couldn't register the panel shortcut"),
    }
    outcome
}

/// Registers the configured shortcut at start. Failures show on the Shortcuts page.
pub fn register_configured(app: &AppHandle, configured: Option<&str>) {
    if supported() {
        let _ = register_panel(app, configured.unwrap_or(DEFAULT_PANEL));
    }
}

pub fn view(configured: Option<&str>) -> ShortcutsView {
    if !supported() {
        return ShortcutsView { supported: false, panel: None, default_panel: None, error: None, warning: None };
    }
    let panel = configured.unwrap_or(DEFAULT_PANEL).to_owned();
    ShortcutsView {
        supported: true,
        warning: normalize(&panel).ok().and_then(|normalized| clash(&normalized)),
        panel: Some(panel),
        default_panel: Some(DEFAULT_PANEL.to_owned()),
        error: LAST_ERROR.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_and_validates() {
        assert_eq!(normalize("cmd+shift+v").unwrap(), "Shift+Command+V");
        assert_eq!(normalize("Control + Alt + KeyK").unwrap(), "Control+Alt+K");
        assert_eq!(normalize("F5").unwrap(), "F5");
        assert_eq!(normalize("Command+Digit1").unwrap(), "Command+1");
        assert!(normalize("V").is_err(), "a bare letter would fire while typing");
        assert!(normalize("Shift+Command").is_err(), "no key");
        assert!(normalize("Command+V+B").is_err(), "two keys");
        assert!(normalize("Command+🙂").is_err());
    }

    #[test]
    fn refuses_reserved_and_flags_clashes() {
        assert!(check_reserved("Command+Space").is_err());
        assert!(check_reserved("Shift+Command+V").is_ok());
        assert!(clash("Shift+Command+V").is_some_and(|note| note.contains("Paste and Match Style")));
        assert!(clash("Control+Alt+K").is_none());
        assert_eq!(display("Shift+Command+V"), "⇧⌘V");
    }

    #[test]
    fn the_default_parses_as_an_accelerator() {
        let normalized = normalize(DEFAULT_PANEL).unwrap();
        assert!(normalized.parse::<Shortcut>().is_ok());
        assert!(normalize("Control+Alt+Shift+Command+F12").unwrap().parse::<Shortcut>().is_ok());
    }
}

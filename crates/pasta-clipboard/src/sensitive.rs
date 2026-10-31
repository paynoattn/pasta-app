//! Built-in list of password managers whose copies are never synced.
//!
//! Not every manager marks its copies as concealed (ADR 0005: Proton Pass and
//! Apple Passwords don't), so watchers also skip copies made in these apps.
//! The list becomes user-editable with excluded apps (#36).

/// macOS bundle ids. Proton Pass and Apple Passwords are confirmed (ADR 0005);
/// the others are their published bundle ids, not yet tested.
pub const MACOS_BUNDLE_IDS: &[&str] = &[
    "me.proton.pass.electron",
    "com.apple.Passwords",
    "com.apple.keychainaccess",
    "com.1password.1password",
    "com.agilebits.onepassword7",
    "com.bitwarden.desktop",
    "org.keepassxc.keepassxc",
];

/// Windows executable names, lowercase. These apps also set the clipboard
/// exclusion formats; the list is a second line of defence. Not yet tested.
pub const WINDOWS_EXES: &[&str] =
    &["1password.exe", "bitwarden.exe", "keepassxc.exe", "keepass.exe", "proton pass.exe", "protonpass.exe"];

/// Chromium extension ids, matched against the `chrome-extension://<id>`
/// origin in `org.chromium.source-url`. Proton Pass is confirmed (ADR 0005).
pub const CHROMIUM_EXTENSION_IDS: &[&str] = &["ghmbeldphafepmbegfdlkpapadhbakde"];

pub fn is_password_manager_bundle(bundle_id: &str) -> bool {
    MACOS_BUNDLE_IDS.contains(&bundle_id)
}

pub fn is_password_manager_exe(exe: &str) -> bool {
    WINDOWS_EXES.iter().any(|known| known.eq_ignore_ascii_case(exe))
}

/// True if a copy's source URL is a known password-manager extension.
pub fn is_password_manager_source_url(url: &str) -> bool {
    url.strip_prefix("chrome-extension://")
        .and_then(|rest| rest.split(['/', '?', '#']).next())
        .is_some_and(|id| CHROMIUM_EXTENSION_IDS.contains(&id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_known_managers() {
        assert!(is_password_manager_bundle("com.apple.Passwords"));
        assert!(!is_password_manager_bundle("com.apple.Notes"));
        assert!(is_password_manager_exe("Bitwarden.exe"));
        assert!(!is_password_manager_exe("notepad.exe"));
        assert!(is_password_manager_source_url("chrome-extension://ghmbeldphafepmbegfdlkpapadhbakde/popup.html"));
        assert!(is_password_manager_source_url("chrome-extension://ghmbeldphafepmbegfdlkpapadhbakde"));
        assert!(!is_password_manager_source_url("chrome-extension://otherextensionid/popup.html"));
        assert!(!is_password_manager_source_url("https://ghmbeldphafepmbegfdlkpapadhbakde.example"));
    }
}

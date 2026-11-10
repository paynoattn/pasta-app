//! The Tauri commands the UI calls (#28). Typed wrappers live in `src/ipc/`,
//! and the types they return are generated from `crate::views`.

use std::sync::Arc;

use pasta_core::config::ExcludedApp;
use pasta_core::device::SyncMode;
use serde::Deserialize;
use tauri::{AppHandle, Manager, State};
use ts_rs::TS;
use uuid::Uuid;

use crate::AppState;
use crate::chain::ChainView;
use crate::clip_history::PasteOutcome;
use crate::runtime::Runtime;
use crate::setup::SettingsLink;
use crate::views::{
    AppInfo, ExcludedAppView, ExpiryView, HistoryFilterView, HistoryItemView, Mode, PeerView, SettingsView, SetupView,
    ShortcutsView, StartupView, StatusView,
};

type Result<T> = std::result::Result<T, String>;

fn runtime(state: &State<'_, AppState>) -> Result<Arc<Runtime>> {
    state.runtime.get().cloned().ok_or_else(|| "Pasta is still starting (is the keychain locked?)".to_owned())
}

fn item_id(id: &str) -> Result<Uuid> {
    id.parse().map_err(|_| "not an item id".to_owned())
}

#[tauri::command]
pub fn app_info() -> AppInfo {
    AppInfo::current()
}

/// Known devices, connected first. Updates arrive as `peers-changed`.
#[tauri::command]
pub fn list_peers(state: State<'_, AppState>) -> Vec<PeerView> {
    state.runtime.get().map(|runtime| runtime.peer_views()).unwrap_or_default()
}

// -- Sync chain (#20, #22, #46) ------------------------------------------------

#[tauri::command]
pub fn chain_status(state: State<'_, AppState>) -> Result<ChainView> {
    Ok(runtime(&state)?.chain_view())
}

/// Starts a new chain. Returns its 24 words.
#[tauri::command]
pub async fn create_chain(state: State<'_, AppState>) -> Result<String> {
    let words = runtime(&state)?.create_chain().await?;
    Ok(words.as_str().to_owned())
}

#[tauri::command]
pub async fn join_chain(state: State<'_, AppState>, words: String) -> Result<()> {
    let words = zeroize::Zeroizing::new(words);
    runtime(&state)?.join_chain(&words).await
}

#[tauri::command]
pub async fn leave_chain(state: State<'_, AppState>) -> Result<()> {
    runtime(&state)?.leave_chain().await
}

/// The chain's 24 words, for adding another device.
#[tauri::command]
pub fn chain_words(state: State<'_, AppState>) -> Result<String> {
    Ok(runtime(&state)?.chain_words()?.as_str().to_owned())
}

#[tauri::command]
pub fn chain_qr(state: State<'_, AppState>) -> Result<String> {
    runtime(&state)?.chain_qr()
}

#[tauri::command]
pub async fn copy_chain_words(state: State<'_, AppState>) -> Result<()> {
    runtime(&state)?.copy_chain_words().await
}

/// `key` is the fingerprint shown for the device, which the user compared with its screen.
#[tauri::command]
pub async fn approve_device(state: State<'_, AppState>, device_id: String, key: String) -> Result<()> {
    runtime(&state)?.approve_device(crate::views::device_id(&device_id)?, &key).await
}

#[tauri::command]
pub async fn decline_device(state: State<'_, AppState>, device_id: String) -> Result<()> {
    runtime(&state)?.decline_device(crate::views::device_id(&device_id)?).await
}

#[tauri::command]
pub async fn remove_device(state: State<'_, AppState>, device_id: String) -> Result<()> {
    runtime(&state)?.remove_device(crate::views::device_id(&device_id)?).await
}

// -- Settings (#29–#32, #36, #41) ----------------------------------------------

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Result<SettingsView> {
    Ok(SettingsView::new(&runtime(&state)?.config.get()))
}

/// Sets any member's mode and expiry; another device gets it now or when it reconnects (#31).
#[tauri::command]
pub fn set_device_settings(
    state: State<'_, AppState>,
    device_id: String,
    mode: Mode,
    ttl_secs: Option<u64>,
) -> Result<()> {
    runtime(&state)?.set_device_settings(crate::views::device_id(&device_id)?, SyncMode::from(mode), ttl_secs)
}

#[tauri::command]
pub fn pause_for_hour(state: State<'_, AppState>) -> Result<()> {
    runtime(&state)?.pause_for_hour()
}

#[tauri::command]
pub fn resume_sync(state: State<'_, AppState>) -> Result<()> {
    runtime(&state)?.resume()
}

#[tauri::command]
pub fn set_privacy(
    state: State<'_, AppState>,
    skip_sensitive: bool,
    excluded_apps: Vec<ExcludedAppView>,
) -> Result<()> {
    let apps = excluded_apps.into_iter().map(|app| ExcludedApp { id: app.id, name: app.name }).collect();
    runtime(&state)?.set_privacy(skip_sensitive, apps)
}

/// Apps with a window open, for the excluded-apps picker.
#[tauri::command]
pub async fn running_apps() -> Vec<ExcludedAppView> {
    tauri::async_runtime::spawn_blocking(pasta_clipboard::running_apps)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|app| ExcludedAppView { id: app.id, name: app.name })
        .collect()
}

#[tauri::command]
pub fn set_add_to_history(state: State<'_, AppState>, on: bool) -> Result<()> {
    runtime(&state)?.set_add_to_history(on)
}

#[tauri::command]
pub fn set_history_prefs(state: State<'_, AppState>, max_items: usize, keep_days: Option<u32>) -> Result<()> {
    runtime(&state)?.set_history_prefs(max_items, keep_days)
}

#[tauri::command]
pub fn set_icloud_bridge(state: State<'_, AppState>, on: bool) -> Result<()> {
    runtime(&state)?.set_icloud_bridge(on)
}

#[tauri::command]
pub fn set_device_name(state: State<'_, AppState>, name: Option<String>) -> Result<()> {
    runtime(&state)?.set_device_name(name)
}

// -- Expiry (#33, #34) ---------------------------------------------------------

#[tauri::command]
pub fn expiry_list(state: State<'_, AppState>) -> Vec<ExpiryView> {
    state.runtime.get().map(|runtime| runtime.expiry_views()).unwrap_or_default()
}

/// "Expire all now". Returns how many items went.
#[tauri::command]
pub async fn expire_all_now(state: State<'_, AppState>) -> Result<usize> {
    runtime(&state)?.expire_all().await
}

/// "Clear from Win + V". Returns how many items went.
#[tauri::command]
pub async fn clear_from_history(state: State<'_, AppState>) -> Result<usize> {
    runtime(&state)?.expire_all().await
}

// -- History and the panel (#37–#40) -------------------------------------------

#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HistoryRequest {
    pub text: Option<String>,
    #[serde(default)]
    pub filter: HistoryFilterView,
    #[ts(type = "number")]
    pub limit: usize,
}

#[tauri::command]
pub fn history_list(state: State<'_, AppState>, request: HistoryRequest) -> Result<Vec<HistoryItemView>> {
    runtime(&state)?.history_items(request.text, request.filter, request.limit)
}

#[tauri::command]
pub async fn history_paste(state: State<'_, AppState>, id: String, plain: bool) -> Result<PasteOutcome> {
    runtime(&state)?.paste_item(item_id(&id)?, plain).await
}

#[tauri::command]
pub async fn history_copy(state: State<'_, AppState>, id: String, plain: bool) -> Result<()> {
    runtime(&state)?.copy_item(item_id(&id)?, plain).await
}

#[tauri::command]
pub fn history_delete(state: State<'_, AppState>, id: String) -> Result<()> {
    runtime(&state)?.delete_item(item_id(&id)?)
}

#[tauri::command]
pub fn history_clear(state: State<'_, AppState>) -> Result<usize> {
    runtime(&state)?.clear_history()
}

#[tauri::command]
pub async fn app_icon(state: State<'_, AppState>, bundle_id: String) -> Result<Option<String>> {
    if bundle_id.len() > 256 {
        return Ok(None);
    }
    Ok(runtime(&state)?.app_icon(bundle_id).await)
}

#[tauri::command]
pub fn hide_panel(app: AppHandle) {
    crate::ui::hide_panel(&app);
}

// -- Tray, windows and the app (#42) -------------------------------------------

#[tauri::command]
pub fn status(state: State<'_, AppState>) -> StatusView {
    match state.runtime.get() {
        Some(runtime) => runtime.status_view(),
        None => state.boot_status(),
    }
}

/// A window the UI can open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum WindowName {
    Settings,
    Setup,
    Panel,
}

/// Async on purpose: a sync command runs on the main thread, and building a
/// window there (the setup window, the first time) deadlocks WebView2 on
/// Windows, which hangs every window's IPC (tauri-apps/wry#583).
#[tauri::command]
pub async fn open_window(app: AppHandle, window: WindowName, section: Option<String>) {
    match window {
        WindowName::Settings => crate::ui::open_settings(&app, section.as_deref()),
        WindowName::Setup => crate::ui::open_setup(&app),
        WindowName::Panel => {
            crate::ui::hide_tray(&app);
            crate::ui::toggle_panel(&app);
        }
    }
}

#[tauri::command]
pub fn quit_app(app: AppHandle) {
    crate::quit(&app);
}

#[tauri::command]
pub fn retry_keychain(state: State<'_, AppState>) {
    state.retry.notify_one();
}

#[tauri::command]
pub async fn copy_diagnostics(state: State<'_, AppState>) -> Result<()> {
    runtime(&state)?.copy_diagnostics().await;
    Ok(())
}

/// The redacted report "Copy diagnostics" puts on the clipboard, for Settings › About.
#[tauri::command]
pub fn diagnostics_report(state: State<'_, AppState>) -> Result<String> {
    Ok(crate::diagnostics::report(&*runtime(&state)?))
}

#[tauri::command]
pub fn open_log_folder(app: AppHandle) -> Result<()> {
    let dir = app.path().app_log_dir().map_err(|err| err.to_string())?;
    crate::tray::open_folder(&dir);
    Ok(())
}

// -- Setup (#43–#45) -----------------------------------------------------------

#[tauri::command]
pub fn setup_status(state: State<'_, AppState>) -> Result<SetupView> {
    Ok(runtime(&state)?.setup_view())
}

#[tauri::command]
pub fn open_settings_link(link: SettingsLink) -> Result<()> {
    crate::setup::open_link(link)
}

/// macOS: asks for the Accessibility permission. Returns whether it's granted.
#[tauri::command]
pub fn request_accessibility() -> Option<bool> {
    crate::setup::request_accessibility()
}

/// Marks setup done and closes its window.
#[tauri::command]
pub fn finish_setup(app: AppHandle, state: State<'_, AppState>) -> Result<()> {
    runtime(&state)?.finish_setup()?;
    crate::ui::close_setup(&app);
    Ok(())
}

// -- Startup and updates (#47) -------------------------------------------------

#[tauri::command]
pub fn startup_status(app: AppHandle, state: State<'_, AppState>) -> Result<StartupView> {
    let updates = runtime(&state)?.config.get().updates;
    Ok(crate::startup::view(&app, updates.automatic, updates.check_hours))
}

#[tauri::command]
pub fn set_open_at_login(app: AppHandle, on: bool) -> Result<()> {
    crate::startup::set_open_at_login(&app, on)
}

#[tauri::command]
pub fn set_update_prefs(state: State<'_, AppState>, automatic: bool, check_hours: u32) -> Result<()> {
    if !(1..=24 * 7).contains(&check_hours) {
        return Err("check between every hour and every week".into());
    }
    runtime(&state)?
        .config
        .update(|config| {
            config.updates.automatic = automatic;
            config.updates.check_hours = check_hours;
        })
        .map_err(|err| err.to_string())
}

/// Checks for an update now. Returns the newer version, if any.
#[tauri::command]
pub async fn check_updates(app: AppHandle, install: bool) -> Result<Option<String>> {
    crate::startup::check(&app, install).await
}

// -- Shortcuts (#48) -----------------------------------------------------------

#[tauri::command]
pub fn shortcuts_status(state: State<'_, AppState>) -> Result<ShortcutsView> {
    Ok(crate::shortcuts::view(runtime(&state)?.config.get().panel_shortcut.as_deref()))
}

/// Sets the panel shortcut; `null` goes back to the default. Only saved once it registers.
#[tauri::command]
pub fn set_panel_shortcut(
    app: AppHandle,
    state: State<'_, AppState>,
    accelerator: Option<String>,
) -> Result<ShortcutsView> {
    if !crate::shortcuts::supported() {
        return Err("there's no clipboard panel on this OS yet".into());
    }
    let runtime = runtime(&state)?;
    let previous = runtime.config.get().panel_shortcut;
    let wanted = match accelerator {
        Some(accelerator) => Some(crate::shortcuts::normalize(&accelerator)?),
        None => None,
    };
    let target = wanted.as_deref().unwrap_or(crate::shortcuts::DEFAULT_PANEL);
    if let Err(err) = crate::shortcuts::register_panel(&app, target) {
        // Keep the old one working.
        crate::shortcuts::register_configured(&app, previous.as_deref());
        return Err(err);
    }
    runtime.config.update(|config| config.panel_shortcut = wanted.clone()).map_err(|err| err.to_string())?;
    Ok(crate::shortcuts::view(wanted.as_deref()))
}

/// Pauses or resumes the panel shortcut while the Shortcuts page records a new one.
#[tauri::command]
pub fn suspend_shortcut(app: AppHandle, state: State<'_, AppState>, suspended: bool) -> Result<()> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt as _;
    if suspended {
        app.global_shortcut().unregister_all().map_err(|err| err.to_string())
    } else {
        crate::shortcuts::register_configured(&app, runtime(&state)?.config.get().panel_shortcut.as_deref());
        Ok(())
    }
}

/// Every command, for `generate_handler!`.
#[macro_export]
macro_rules! all_commands {
    () => {
        tauri::generate_handler![
            $crate::commands::app_info,
            $crate::commands::list_peers,
            $crate::commands::chain_status,
            $crate::commands::create_chain,
            $crate::commands::join_chain,
            $crate::commands::leave_chain,
            $crate::commands::chain_words,
            $crate::commands::chain_qr,
            $crate::commands::copy_chain_words,
            $crate::commands::approve_device,
            $crate::commands::decline_device,
            $crate::commands::remove_device,
            $crate::commands::get_settings,
            $crate::commands::set_device_settings,
            $crate::commands::pause_for_hour,
            $crate::commands::resume_sync,
            $crate::commands::set_privacy,
            $crate::commands::running_apps,
            $crate::commands::set_add_to_history,
            $crate::commands::set_history_prefs,
            $crate::commands::set_icloud_bridge,
            $crate::commands::set_device_name,
            $crate::commands::expiry_list,
            $crate::commands::expire_all_now,
            $crate::commands::clear_from_history,
            $crate::commands::history_list,
            $crate::commands::history_paste,
            $crate::commands::history_copy,
            $crate::commands::history_delete,
            $crate::commands::history_clear,
            $crate::commands::app_icon,
            $crate::commands::hide_panel,
            $crate::commands::status,
            $crate::commands::open_window,
            $crate::commands::quit_app,
            $crate::commands::retry_keychain,
            $crate::commands::copy_diagnostics,
            $crate::commands::diagnostics_report,
            $crate::commands::open_log_folder,
            $crate::commands::setup_status,
            $crate::commands::open_settings_link,
            $crate::commands::request_accessibility,
            $crate::commands::finish_setup,
            $crate::commands::startup_status,
            $crate::commands::set_open_at_login,
            $crate::commands::set_update_prefs,
            $crate::commands::check_updates,
            $crate::commands::shortcuts_status,
            $crate::commands::set_panel_shortcut,
            $crate::commands::suspend_shortcut,
        ]
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_checked() {
        assert!(item_id("not-a-uuid").is_err());
        assert!(item_id(&Uuid::new_v4().to_string()).is_ok());
        assert!(crate::views::device_id("x").is_err());
    }
}

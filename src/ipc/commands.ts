/**
 * Typed wrappers around Pasta's Tauri commands and events (#28).
 *
 * The types come from `./bindings`, which `cargo test -p pasta-app` generates
 * from the Rust views (ts-rs), so a change on either side shows up as a type
 * error. Errors from Rust arrive as plain strings.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { AppInfo } from "./bindings/AppInfo";
import type { ExcludedAppView } from "./bindings/ExcludedAppView";
import type { ExpiryView } from "./bindings/ExpiryView";
import type { HistoryFilterView } from "./bindings/HistoryFilterView";
import type { HistoryItemView } from "./bindings/HistoryItemView";
import type { Mode } from "./bindings/Mode";
import type { PanelContext } from "./bindings/PanelContext";
import type { PasteOutcome } from "./bindings/PasteOutcome";
import type { PeerView } from "./bindings/PeerView";
import type { SettingsLink } from "./bindings/SettingsLink";
import type { SettingsView } from "./bindings/SettingsView";
import type { SetupView } from "./bindings/SetupView";
import type { ShortcutsView } from "./bindings/ShortcutsView";
import type { StartupView } from "./bindings/StartupView";
import type { StatusView } from "./bindings/StatusView";
import type { WindowName } from "./bindings/WindowName";

export type {
  AppInfo,
  ExcludedAppView,
  ExpiryView,
  HistoryFilterView,
  HistoryItemView,
  Mode,
  PanelContext,
  PasteOutcome,
  PeerView,
  SettingsLink,
  SettingsView,
  SetupView,
  ShortcutsView,
  StartupView,
  StatusView,
  WindowName,
};
export type { DeviceSettingsView } from "./bindings/DeviceSettingsView";
export type { ItemKindView } from "./bindings/ItemKindView";
export type { TailscaleView } from "./bindings/TailscaleView";

/** A row in the Devices list. */
export type Peer = PeerView;

/** Event names, as emitted by `src-tauri/src/runtime.rs` and friends. */
export const EVENTS = {
  peers: "peers-changed",
  chain: "chain-changed",
  settings: "settings-changed",
  history: "history-changed",
  status: "status-changed",
  expiry: "expiry-changed",
  update: "update-status",
  navigate: "navigate",
  panelOpened: "panel-opened",
  trayOpened: "tray-opened",
} as const;

export type EventName = (typeof EVENTS)[keyof typeof EVENTS];

/** Subscribes to one event; resolves to the function that unsubscribes. */
export function on<T>(event: EventName, handler: (payload: T) => void): Promise<UnlistenFn> {
  return listen<T>(event, (message) => handler(message.payload));
}

export function getAppInfo(): Promise<AppInfo> {
  return invoke<AppInfo>("app_info");
}

/** Known devices, connected first. */
export function listPeers(): Promise<Peer[]> {
  return invoke<Peer[]>("list_peers");
}

/** Calls `handler` with the full list whenever a device connects, disconnects or reports latency. */
export function onPeersChanged(handler: (peers: Peer[]) => void): Promise<UnlistenFn> {
  return listen<Peer[]>("peers-changed", (event) => handler(event.payload));
}

// -- Settings ------------------------------------------------------------------

export function getSettings(): Promise<SettingsView> {
  return invoke<SettingsView>("get_settings");
}

/** Any member's mode and expiry. Another device gets it now, or when it reconnects. */
export function setDeviceSettings(
  deviceId: string,
  mode: Mode,
  ttlSecs: number | null,
): Promise<void> {
  return invoke<void>("set_device_settings", { deviceId, mode, ttlSecs });
}

export function pauseForHour(): Promise<void> {
  return invoke<void>("pause_for_hour");
}

export function resumeSync(): Promise<void> {
  return invoke<void>("resume_sync");
}

export function setPrivacy(skipSensitive: boolean, excludedApps: ExcludedAppView[]): Promise<void> {
  return invoke<void>("set_privacy", { skipSensitive, excludedApps });
}

/** Apps with a window open, for the excluded-apps picker. */
export function runningApps(): Promise<ExcludedAppView[]> {
  return invoke<ExcludedAppView[]>("running_apps");
}

export function setAddToHistory(on: boolean): Promise<void> {
  return invoke<void>("set_add_to_history", { on });
}

export function setHistoryPrefs(maxItems: number, keepDays: number | null): Promise<void> {
  return invoke<void>("set_history_prefs", { maxItems, keepDays });
}

export function setIcloudBridge(on: boolean): Promise<void> {
  return invoke<void>("set_icloud_bridge", { on });
}

export function setDeviceName(name: string | null): Promise<void> {
  return invoke<void>("set_device_name", { name });
}

// -- Expiry --------------------------------------------------------------------

export function expiryList(): Promise<ExpiryView[]> {
  return invoke<ExpiryView[]>("expiry_list");
}

/** Resolves to how many items were expired. */
export function expireAllNow(): Promise<number> {
  return invoke<number>("expire_all_now");
}

/** Windows: removes every item Pasta put into Win + V. */
export function clearFromHistory(): Promise<number> {
  return invoke<number>("clear_from_history");
}

// -- History and the panel ------------------------------------------------------

export function historyList(
  text: string | null,
  filter: HistoryFilterView,
  limit: number,
): Promise<HistoryItemView[]> {
  return invoke<HistoryItemView[]>("history_list", { request: { text, filter, limit } });
}

/** Copies the item and pastes it into the app the panel opened over. */
export function historyPaste(id: string, plain: boolean): Promise<PasteOutcome> {
  return invoke<PasteOutcome>("history_paste", { id, plain });
}

export function historyCopy(id: string, plain: boolean): Promise<void> {
  return invoke<void>("history_copy", { id, plain });
}

export function historyDelete(id: string): Promise<void> {
  return invoke<void>("history_delete", { id });
}

export function historyClear(): Promise<number> {
  return invoke<number>("history_clear");
}

/** A `data:` URL of the app's icon, or `null`. */
export function appIcon(bundleId: string): Promise<string | null> {
  return invoke<string | null>("app_icon", { bundleId });
}

export function hidePanel(): Promise<void> {
  return invoke<void>("hide_panel");
}

// -- Tray and windows ----------------------------------------------------------

export function getStatus(): Promise<StatusView> {
  return invoke<StatusView>("status");
}

export function openWindow(window: WindowName, section?: string): Promise<void> {
  return invoke<void>("open_window", { window, section: section ?? null });
}

export function quitApp(): Promise<void> {
  return invoke<void>("quit_app");
}

export function retryKeychain(): Promise<void> {
  return invoke<void>("retry_keychain");
}

export function copyDiagnostics(): Promise<void> {
  return invoke<void>("copy_diagnostics");
}

/** The redacted report "Copy diagnostics" puts on the clipboard. */
export function diagnosticsReport(): Promise<string> {
  return invoke<string>("diagnostics_report");
}

export function openLogFolder(): Promise<void> {
  return invoke<void>("open_log_folder");
}

// -- Setup ---------------------------------------------------------------------

export function setupStatus(): Promise<SetupView> {
  return invoke<SetupView>("setup_status");
}

export function openSettingsLink(link: SettingsLink): Promise<void> {
  return invoke<void>("open_settings_link", { link });
}

/** macOS: shows the system's Accessibility prompt. Resolves to whether it's granted. */
export function requestAccessibility(): Promise<boolean | null> {
  return invoke<boolean | null>("request_accessibility");
}

export function finishSetup(): Promise<void> {
  return invoke<void>("finish_setup");
}

// -- Startup and updates ---------------------------------------------------------

export function startupStatus(): Promise<StartupView> {
  return invoke<StartupView>("startup_status");
}

export function setOpenAtLogin(on: boolean): Promise<void> {
  return invoke<void>("set_open_at_login", { on });
}

export function setUpdatePrefs(automatic: boolean, checkHours: number): Promise<void> {
  return invoke<void>("set_update_prefs", { automatic, checkHours });
}

/** Resolves to the newer version, or `null` if Pasta is up to date. */
export function checkUpdates(install: boolean): Promise<string | null> {
  return invoke<string | null>("check_updates", { install });
}

// -- Shortcuts -----------------------------------------------------------------

export function shortcutsStatus(): Promise<ShortcutsView> {
  return invoke<ShortcutsView>("shortcuts_status");
}

/** `null` goes back to the default. Rejects with the reason if it can't be registered. */
export function setPanelShortcut(accelerator: string | null): Promise<ShortcutsView> {
  return invoke<ShortcutsView>("set_panel_shortcut", { accelerator });
}

/** Stops the panel shortcut firing while a new one is recorded. */
export function suspendShortcut(suspended: boolean): Promise<void> {
  return invoke<void>("suspend_shortcut", { suspended });
}

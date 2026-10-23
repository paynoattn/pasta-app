import { invoke } from "@tauri-apps/api/core";

/** Mirrors `AppInfo` in `src-tauri/src/lib.rs`. */
export interface AppInfo {
  version: string;
  protocolVersion: number;
}

export function getAppInfo(): Promise<AppInfo> {
  return invoke<AppInfo>("app_info");
}

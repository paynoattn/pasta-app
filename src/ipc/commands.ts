import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** Mirrors `AppInfo` in `src-tauri/src/lib.rs`. */
export interface AppInfo {
  version: string;
  protocolVersion: number;
}

export function getAppInfo(): Promise<AppInfo> {
  return invoke<AppInfo>("app_info");
}

/** Mirrors `PeerView` in `src-tauri/src/runtime.rs`. */
export interface Peer {
  deviceId: string;
  name: string;
  os: string;
  connected: boolean;
  latencyMs: number | null;
  /** Milliseconds since the Unix epoch. */
  lastSeen: number;
  /** `member`, `pending` (in the chain, not approved) or `unknown` (this device waits for approval). */
  trust: "member" | "pending" | "unknown";
}

/** Known devices, connected first. */
export function listPeers(): Promise<Peer[]> {
  return invoke<Peer[]>("list_peers");
}

/** Calls `handler` with the full list whenever a device connects, disconnects or reports latency. */
export function onPeersChanged(handler: (peers: Peer[]) => void): Promise<UnlistenFn> {
  return listen<Peer[]>("peers-changed", (event) => handler(event.payload));
}

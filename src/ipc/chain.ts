import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

/** Mirrors `MemberView` in `src-tauri/src/chain.rs`. */
export interface ChainMember {
  deviceId: string;
  name: string;
  os: string;
  thisDevice: boolean;
  connected: boolean;
}

/** Mirrors `PendingView` in `src-tauri/src/chain.rs`. */
export interface PendingDevice {
  deviceId: string;
  name: string;
  os: string;
  /** The Tailscale device it connected from. */
  nodeName: string;
  /** First 16 hex digits of its device key; the joining device shows the same. */
  key: string;
  wasRemoved: boolean;
}

/** Mirrors `ChainView` in `src-tauri/src/chain.rs`. */
export interface ChainView {
  /** `none`: not in a chain. `waiting`: knows the words, no member has approved it yet. */
  state: "none" | "waiting" | "member";
  /** `ABCD-EFGH`, the same on every device in the chain. */
  fingerprint: string | null;
  /** First 16 hex digits of this device's key. */
  thisKey: string;
  members: ChainMember[];
  pending: PendingDevice[];
}

export function getChain(): Promise<ChainView> {
  return invoke<ChainView>("chain_status");
}

export function onChainChanged(handler: (chain: ChainView) => void): Promise<UnlistenFn> {
  return listen<ChainView>("chain-changed", (event) => handler(event.payload));
}

/** Starts a new chain with this device as its first member. Resolves to the 24 words. */
export function createChain(): Promise<string> {
  return invoke<string>("create_chain");
}

/** Joins the chain the words belong to; another member then approves this device. */
export function joinChain(words: string): Promise<void> {
  return invoke<void>("join_chain", { words });
}

export function leaveChain(): Promise<void> {
  return invoke<void>("leave_chain");
}

/** The 24 words, for adding another device. */
export function chainWords(): Promise<string> {
  return invoke<string>("chain_words");
}

/** `key` is the fingerprint shown for the device, compared with the joining device's screen. */
export function approveDevice(deviceId: string, key: string): Promise<void> {
  return invoke<void>("approve_device", { deviceId, key });
}

export function declineDevice(deviceId: string): Promise<void> {
  return invoke<void>("decline_device", { deviceId });
}

export function removeDevice(deviceId: string): Promise<void> {
  return invoke<void>("remove_device", { deviceId });
}

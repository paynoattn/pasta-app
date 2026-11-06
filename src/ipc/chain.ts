import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type { ChainView } from "./bindings/ChainView";
import type { MemberView } from "./bindings/MemberView";
import type { PendingView } from "./bindings/PendingView";

export type { ChainView };
/** A member of the chain. */
export type ChainMember = MemberView;
/** A device that knows the words and waits for a member to approve it. */
export type PendingDevice = PendingView;

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

/** The words as a QR code: an SVG `data:` URL. */
export function chainQr(): Promise<string> {
  return invoke<string>("chain_qr");
}

/** Copies the words, marked concealed and kept out of every clipboard history, without syncing. */
export function copyChainWords(): Promise<void> {
  return invoke<void>("copy_chain_words");
}

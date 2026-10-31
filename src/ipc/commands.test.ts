import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { describe, expect, it, vi } from "vitest";
import { getAppInfo, listPeers, onPeersChanged, type Peer } from "./commands";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const peer: Peer = {
  deviceId: "00000000-0000-4000-8000-000000000001",
  name: "macbook-pro",
  os: "macos",
  connected: true,
  latencyMs: 12,
  lastSeen: 1730000000000,
};

describe("getAppInfo", () => {
  it("calls the app_info command and returns its result", async () => {
    vi.mocked(invoke).mockResolvedValue({ version: "0.1.0", protocolVersion: 1 });

    await expect(getAppInfo()).resolves.toEqual({ version: "0.1.0", protocolVersion: 1 });
    expect(invoke).toHaveBeenCalledWith("app_info");
  });
});

describe("listPeers", () => {
  it("calls the list_peers command", async () => {
    vi.mocked(invoke).mockResolvedValue([peer]);

    await expect(listPeers()).resolves.toEqual([peer]);
    expect(invoke).toHaveBeenCalledWith("list_peers");
  });
});

describe("onPeersChanged", () => {
  it("passes each peers-changed payload to the handler", async () => {
    const unlisten = vi.fn();
    vi.mocked(listen).mockImplementation((_event, callback) => {
      callback({ event: "peers-changed", id: 1, payload: [peer] });
      return Promise.resolve(unlisten);
    });
    const handler = vi.fn();

    await expect(onPeersChanged(handler)).resolves.toBe(unlisten);
    expect(listen).toHaveBeenCalledWith("peers-changed", expect.any(Function));
    expect(handler).toHaveBeenCalledWith([peer]);
  });
});

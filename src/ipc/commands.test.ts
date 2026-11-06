import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  EVENTS,
  getAppInfo,
  historyList,
  historyPaste,
  listPeers,
  on,
  onPeersChanged,
  openWindow,
  pauseForHour,
  setDeviceSettings,
  setHistoryPrefs,
  setPanelShortcut,
  setPrivacy,
  type Peer,
} from "./commands";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(listen).mockReset();
});

const peer: Peer = {
  deviceId: "00000000-0000-4000-8000-000000000001",
  name: "macbook-pro",
  os: "macos",
  connected: true,
  latencyMs: 12,
  lastSeen: 1730000000000,
  trust: "member",
  address: null,
  settings: null,
  pendingSettings: null,
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

describe("setDeviceSettings", () => {
  it("sends the device id, mode and expiry as separate keys", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);

    await expect(setDeviceSettings(peer.deviceId, "receive", 3600)).resolves.toBeUndefined();
    expect(invoke).toHaveBeenCalledExactlyOnceWith("set_device_settings", {
      deviceId: peer.deviceId,
      mode: "receive",
      ttlSecs: 3600,
    });
  });

  it("passes a null expiry for Never", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);

    await setDeviceSettings(peer.deviceId, "both", null);
    expect(invoke).toHaveBeenCalledWith("set_device_settings", {
      deviceId: peer.deviceId,
      mode: "both",
      ttlSecs: null,
    });
  });
});

describe("pauseForHour", () => {
  it("calls pause_for_hour without arguments", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);

    await expect(pauseForHour()).resolves.toBeUndefined();
    expect(invoke).toHaveBeenCalledExactlyOnceWith("pause_for_hour");
  });
});

describe("setPrivacy", () => {
  it("sends the toggle and the excluded apps", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);

    await expect(
      setPrivacy(true, [{ id: "com.example.secrets", name: "Secrets" }]),
    ).resolves.toBeUndefined();
    expect(invoke).toHaveBeenCalledExactlyOnceWith("set_privacy", {
      skipSensitive: true,
      excludedApps: [{ id: "com.example.secrets", name: "Secrets" }],
    });
  });
});

describe("setHistoryPrefs", () => {
  it("sends both retention numbers", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);

    await expect(setHistoryPrefs(500, 30)).resolves.toBeUndefined();
    expect(invoke).toHaveBeenCalledExactlyOnceWith("set_history_prefs", {
      maxItems: 500,
      keepDays: 30,
    });
  });
});

describe("historyList", () => {
  it("wraps the search, filter and limit in a `request` object", async () => {
    const items = [{ id: "00000000-0000-4000-8000-000000000003", preview: "hi" }];
    vi.mocked(invoke).mockResolvedValue(items);

    await expect(historyList("hi", "links", 200)).resolves.toBe(items);
    expect(invoke).toHaveBeenCalledExactlyOnceWith("history_list", {
      request: { text: "hi", filter: "links", limit: 200 },
    });
  });

  it("passes null text when there is no search yet", async () => {
    vi.mocked(invoke).mockResolvedValue([]);

    await historyList(null, "all", 6);
    expect(invoke).toHaveBeenCalledWith("history_list", {
      request: { text: null, filter: "all", limit: 6 },
    });
  });
});

describe("historyPaste", () => {
  it("sends the item id and whether to paste plain text", async () => {
    vi.mocked(invoke).mockResolvedValue("pasted");

    await expect(historyPaste("00000000-0000-4000-8000-000000000003", false)).resolves.toBe(
      "pasted",
    );
    expect(invoke).toHaveBeenCalledExactlyOnceWith("history_paste", {
      id: "00000000-0000-4000-8000-000000000003",
      plain: false,
    });
  });
});

describe("openWindow", () => {
  it("always sends a section, null when none was asked for", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);

    await openWindow("settings", "add-device");
    expect(invoke).toHaveBeenLastCalledWith("open_window", {
      window: "settings",
      section: "add-device",
    });

    await openWindow("panel");
    expect(invoke).toHaveBeenLastCalledWith("open_window", { window: "panel", section: null });
  });
});

describe("setPanelShortcut", () => {
  it("sends the accelerator, or null for the default", async () => {
    const view = { supported: true, panel: "⇧⌘V", defaultPanel: "⇧⌘V", error: null, warning: null };
    vi.mocked(invoke).mockResolvedValue(view);

    await expect(setPanelShortcut("Shift+Command+V")).resolves.toBe(view);
    expect(invoke).toHaveBeenLastCalledWith("set_panel_shortcut", {
      accelerator: "Shift+Command+V",
    });

    await setPanelShortcut(null);
    expect(invoke).toHaveBeenLastCalledWith("set_panel_shortcut", { accelerator: null });
  });
});

describe("EVENTS", () => {
  it("names every event the UI listens for", () => {
    expect(EVENTS).toEqual({
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
    });
  });
});

describe("on", () => {
  it("subscribes with the mapped event name and unwraps the payload", async () => {
    const unlisten = vi.fn();
    const handler = vi.fn();
    vi.mocked(listen).mockImplementation((_event, callback) => {
      callback({ event: "expiry-changed", id: 1, payload: [1, 2] });
      return Promise.resolve(unlisten);
    });

    await expect(on<number[]>(EVENTS.expiry, handler)).resolves.toBe(unlisten);
    expect(listen).toHaveBeenCalledExactlyOnceWith("expiry-changed", expect.any(Function));
    expect(handler).toHaveBeenCalledWith([1, 2]);
  });
});

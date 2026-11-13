import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as chainIpc from "../../ipc/chain";
import * as commands from "../../ipc/commands";
import type { PeerView } from "../../ipc/bindings/PeerView";
import type { SettingsView } from "../../ipc/bindings/SettingsView";
import { DevicesPage } from "./DevicesPage";

vi.mock("../../ipc/chain", () => ({
  approveDevice: vi.fn(),
  declineDevice: vi.fn(),
  getChain: vi.fn(),
  removeDevice: vi.fn(),
}));
vi.mock("../../ipc/commands", () => ({
  EVENTS: {
    chain: "chain-changed",
    peers: "peers-changed",
    settings: "settings-changed",
    status: "status-changed",
  },
  on: vi.fn().mockResolvedValue(() => {}),
  getSettings: vi.fn(),
  listPeers: vi.fn(),
  openWindow: vi.fn(),
  setDeviceName: vi.fn(),
  setDeviceSettings: vi.fn(),
  setIcloudBridge: vi.fn(),
  setupStatus: vi.fn(),
}));
vi.mock("../../platform/platform", () => ({
  platform: "linux",
  detectPlatform: vi.fn(() => "linux"),
  applyPlatform: vi.fn(),
  modKey: vi.fn(() => "Ctrl"),
  keyStoreName: vi.fn(() => "the Secret Service (KWallet)"),
}));

const getChain = vi.mocked(chainIpc.getChain);
const approveDevice = vi.mocked(chainIpc.approveDevice);
const getSettings = vi.mocked(commands.getSettings);
const listPeers = vi.mocked(commands.listPeers);
const setupStatus = vi.mocked(commands.setupStatus);
const setDeviceName = vi.mocked(commands.setDeviceName);
const setDeviceSettings = vi.mocked(commands.setDeviceSettings);
const setIcloudBridge = vi.mocked(commands.setIcloudBridge);

const THIS_DEVICE = {
  deviceId: "00000000-0000-4000-8000-000000000001",
  name: "macbook-pro",
  os: "macos",
  thisDevice: true,
  connected: true,
};
const PEER_DEVICE = {
  deviceId: "00000000-0000-4000-8000-000000000002",
  name: "win-workstation",
  os: "windows",
  thisDevice: false,
  connected: true,
};

const settingsView: SettingsView = {
  deviceId: THIS_DEVICE.deviceId,
  deviceName: "macbook-pro",
  thisDevice: { mode: "both", effectiveMode: "both", ttlSecs: null, pausedUntil: null },
  ttlChoices: [3600, null],
  addToHistory: true,
  skipSensitive: true,
  excludedApps: [],
  builtinExcluded: [],
  historyMaxItems: 500,
  historyKeepDays: 30,
  icloudBridge: false,
  setupDone: true,
};

const peerView: PeerView = {
  deviceId: PEER_DEVICE.deviceId,
  name: "win-workstation",
  os: "windows",
  connected: true,
  latencyMs: 12,
  lastSeen: 1730000000000,
  trust: "member",
  address: "100.101.102.103",
  settings: { mode: "receive", effectiveMode: "receive", ttlSecs: 3600, pausedUntil: null },
  pendingSettings: null,
};

beforeEach(async () => {
  getChain.mockReset();
  approveDevice.mockReset();
  getSettings.mockReset();
  listPeers.mockReset();
  setupStatus.mockReset();
  setDeviceName.mockReset();
  setDeviceSettings.mockReset();
  setIcloudBridge.mockReset();
  getChain.mockResolvedValue({
    state: "member",
    fingerprint: "ABCD-EFGH",
    thisKey: "a1b2c3d4e5f60718",
    members: [THIS_DEVICE, PEER_DEVICE],
    pending: [],
  });
  getSettings.mockResolvedValue(settingsView);
  listPeers.mockResolvedValue([peerView]);
  setupStatus.mockResolvedValue({
    os: "linux",
    tailscale: { state: "running", detail: null, address: "100.101.102.104", cliOnly: false },
    chain: "member",
    accessibility: null,
    clipboardHistory: null,
    cloudSync: null,
    clipboardUnsupported: null,
    openAtLogin: true,
    autoUpdate: false,
    icloudBridge: false,
    panelShortcut: null,
    setupDone: true,
  });
});

afterEach(cleanup);

async function rendered() {
  render(<DevicesPage addDevice={false} onAddDeviceChange={() => {}} />);
  await screen.findByText("Home sync chain");
}

describe("DevicesPage", () => {
  it("renders the members with the This device badge", async () => {
    await rendered();

    expect(screen.getByText(/2 devices/)).toBeInstanceOf(HTMLElement);
    expect(screen.getByText(/ABCD-EFGH/)).toBeInstanceOf(HTMLElement);
    const badges = screen.getAllByText("This device");
    expect(badges).toHaveLength(1);
    // Both members show as rows with their mode radios.
    expect(screen.getByText("macbook-pro")).toBeInstanceOf(HTMLElement);
    expect(screen.getByText("win-workstation")).toBeInstanceOf(HTMLElement);
    // The peer's tailnet address shows in its hint line.
    expect(screen.getByText(/100\.101\.102\.103/)).toBeInstanceOf(HTMLElement);
  });

  it("changing a peer's mode calls setDeviceSettings with its id, the mode and its expiry", async () => {
    const user = userEvent.setup();
    setDeviceSettings.mockResolvedValue(undefined);
    await rendered();

    // The peer's Segmented group; its current mode is "receive".
    const group = screen.getByRole("radiogroup", { name: "Mode of win-workstation" });
    const send = within(group).getByRole("radio", { name: "Send" });
    await user.click(send);

    await waitFor(() =>
      expect(setDeviceSettings).toHaveBeenCalledExactlyOnceWith(PEER_DEVICE.deviceId, "send", 3600),
    );
  });

  it("changing this device's mode targets its own id and never expires by default", async () => {
    const user = userEvent.setup();
    setDeviceSettings.mockResolvedValue(undefined);
    await rendered();

    const group = screen.getByRole("radiogroup", { name: "Mode of macbook-pro" });
    await user.click(within(group).getByRole("radio", { name: "Paused" }));

    await waitFor(() =>
      expect(setDeviceSettings).toHaveBeenCalledExactlyOnceWith(
        THIS_DEVICE.deviceId,
        "paused",
        null,
      ),
    );
  });

  it("the row menu opens above its button when there's no room below", async () => {
    const user = userEvent.setup();
    const rect = vi
      .spyOn(HTMLElement.prototype, "getBoundingClientRect")
      .mockReturnValue(new DOMRect(900, window.innerHeight - 30, 30, 28));
    const height = vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(40);
    try {
      await rendered();

      await user.click(screen.getByLabelText("More for win-workstation"));
      const menu = screen.getByRole("menu");
      expect(menu.style.bottom).toBe("34px");
      expect(menu.style.top).toBe("");
      expect(within(menu).getByRole("menuitem", { name: "Remove from chain…" })).toBeInstanceOf(
        HTMLElement,
      );
    } finally {
      rect.mockRestore();
      height.mockRestore();
    }
  });

  it("the row menu opens below its button when there's room", async () => {
    const user = userEvent.setup();
    // Anchor at (900, 100), 30×28: 132px of free space below, well within jsdom's 768px height.
    const rect = vi
      .spyOn(HTMLElement.prototype, "getBoundingClientRect")
      .mockReturnValue(new DOMRect(900, 100, 30, 28));
    const height = vi.spyOn(HTMLElement.prototype, "offsetHeight", "get").mockReturnValue(40);
    try {
      await rendered();

      await user.click(screen.getByLabelText("More for win-workstation"));
      const menu = screen.getByRole("menu");
      expect(menu.style.top).toBe("132px");
      expect(menu.style.bottom).toBe("");
      expect(menu.style.right).toBe(`${window.innerWidth - 930}px`);
      expect(within(menu).getByRole("menuitem", { name: "Remove from chain…" })).toBeInstanceOf(
        HTMLElement,
      );
    } finally {
      rect.mockRestore();
      height.mockRestore();
    }
  });

  it("closes the row menu when anything scrolls", async () => {
    const user = userEvent.setup();
    await rendered();

    await user.click(screen.getByLabelText("More for win-workstation"));
    expect(screen.getByRole("menu")).toBeInstanceOf(HTMLElement);

    // The window listener is capturing, so a scroll inside any element closes it.
    fireEvent.scroll(document.body);

    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
  });

  it("closes the row menu when the window resizes", async () => {
    const user = userEvent.setup();
    await rendered();

    await user.click(screen.getByLabelText("More for win-workstation"));
    expect(screen.getByRole("menu")).toBeInstanceOf(HTMLElement);

    window.dispatchEvent(new Event("resize"));

    await waitFor(() => expect(screen.queryByRole("menu")).toBeNull());
  });

  it("hides the iCloud bridge section off macOS", async () => {
    await rendered();
    expect(screen.queryByText("Bridge iCloud Universal Clipboard")).toBeNull();
    expect(setIcloudBridge).not.toHaveBeenCalled();
  });

  it("a pending device gets an approval banner", async () => {
    getChain.mockResolvedValue({
      state: "member",
      fingerprint: "ABCD-EFGH",
      thisKey: "a1b2c3d4e5f60718",
      members: [THIS_DEVICE],
      pending: [
        {
          deviceId: "00000000-0000-4000-8000-000000000003",
          name: "cachy-desktop",
          os: "linux",
          nodeName: "cachy-desktop",
          key: "1122334455667788",
          wasRemoved: false,
        },
      ],
    });
    approveDevice.mockResolvedValue(undefined);
    const user = userEvent.setup();
    await rendered();

    expect(screen.getByText("cachy-desktop wants to join.")).toBeInstanceOf(HTMLElement);
    // Banner buttons are separate from the Add Device dialog's.
    await user.click(screen.getByRole("button", { name: "Approve" }));
    await waitFor(() =>
      expect(approveDevice).toHaveBeenCalledExactlyOnceWith(
        "00000000-0000-4000-8000-000000000003",
        "1122334455667788",
      ),
    );
  });

  it("shows this device's own name once renamed, falling back to the chain member's", async () => {
    getSettings.mockResolvedValue({ ...settingsView, deviceName: "Kitchen Mac" });
    await rendered();

    // The row shows the renamed name, not the chain record's "macbook-pro"…
    expect(screen.getByText("Kitchen Mac")).toBeInstanceOf(HTMLElement);
    expect(screen.queryByText("macbook-pro")).toBeNull();
    // …and the controls follow the shown name.
    expect(screen.getByRole("radiogroup", { name: "Mode of Kitchen Mac" })).toBeInstanceOf(
      HTMLElement,
    );

    // Without a rename, the member's joined name shows. In the app the page
    // reloads on a settings event; here, remounting reads the new mock.
    getSettings.mockResolvedValue({ ...settingsView, deviceName: null });
    cleanup();
    await rendered();
    expect(screen.getByText("macbook-pro")).toBeInstanceOf(HTMLElement);
    expect(screen.queryByText("Kitchen Mac")).toBeNull();
  });

  it("shows another device's announced name when it differs from the chain record", async () => {
    listPeers.mockResolvedValue([{ ...peerView, name: "Work desktop" }]);
    await rendered();

    expect(screen.getByText("Work desktop")).toBeInstanceOf(HTMLElement);
    expect(screen.queryByText("win-workstation")).toBeNull();
    expect(screen.getByRole("combobox", { name: "Expiry on Work desktop" })).toBeInstanceOf(
      HTMLElement,
    );
  });

  it("renames this device: prefills the shown name, keeps focus while typing, saves trimmed", async () => {
    getSettings.mockResolvedValue({ ...settingsView, deviceName: "Kitchen Mac" });
    setDeviceName.mockResolvedValue(undefined);
    const user = userEvent.setup();
    await rendered();

    await user.click(screen.getByRole("button", { name: "More for Kitchen Mac" }));
    await user.click(screen.getByRole("menuitem", { name: "Rename…" }));

    // The input starts with the shown name and focused.
    const input = screen.getByRole("textbox") as HTMLInputElement;
    expect(input.value).toBe("Kitchen Mac");
    expect(document.activeElement).toBe(input);

    await user.clear(input);
    await user.type(input, "  Studio Mac  ");
    // Typing re-renders the row (a fresh onClose each keystroke); focus stays.
    expect(document.activeElement).toBe(input);
    expect(input.value).toBe("  Studio Mac  ");

    await user.click(screen.getByRole("button", { name: "Rename" }));
    await waitFor(() => expect(setDeviceName).toHaveBeenCalledExactlyOnceWith("Studio Mac"));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("expiry options read 'Expire: …' and choosing one saves this device's mode and ttl", async () => {
    setDeviceSettings.mockResolvedValue(undefined);
    const user = userEvent.setup();
    await rendered();

    const select = screen.getByRole("combobox", {
      name: "Expiry on macbook-pro",
    }) as HTMLSelectElement;
    const options = within(screen.getByRole("combobox", { name: "Expiry on macbook-pro" }))
      .getAllByRole("option")
      .map((option) => (option as HTMLOptionElement).textContent);
    // ttlChoices [3600, null] → "1 hour", "Never", prefixed and lowercased.
    expect(options).toEqual(["Expire: 1 hour", "Expire: never"]);

    // This device's mode is "both"; changing only its expiry keeps that mode.
    await user.selectOptions(select, "3600");

    await waitFor(() =>
      expect(setDeviceSettings).toHaveBeenCalledExactlyOnceWith(THIS_DEVICE.deviceId, "both", 3600),
    );
  });

  it("says a peer with pending settings gets them when it reconnects", async () => {
    listPeers.mockResolvedValue([
      {
        ...peerView,
        pendingSettings: { mode: "send", effectiveMode: "send", ttlSecs: 3600, pausedUntil: null },
      },
    ]);
    await rendered();

    expect(screen.getByText(/changes reach it when it reconnects/)).toBeInstanceOf(HTMLElement);
  });
});

import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
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
    tailscale: { state: "running", detail: null, address: "100.101.102.104" },
    chain: "member",
    accessibility: null,
    clipboardHistory: null,
    cloudSync: null,
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
});

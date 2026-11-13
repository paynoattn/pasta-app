import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as commands from "../../ipc/commands";
import type { SettingsView } from "../../ipc/bindings/SettingsView";
import { ClipboardPage } from "./ClipboardPage";

vi.mock("../../ipc/commands", () => ({
  EVENTS: {
    chain: "chain-changed",
    history: "history-changed",
    peers: "peers-changed",
    settings: "settings-changed",
    status: "status-changed",
  },
  on: vi.fn().mockResolvedValue(() => {}),
  clearFromHistory: vi.fn(),
  expireAllNow: vi.fn(),
  getSettings: vi.fn(),
  historyClear: vi.fn(),
  pauseForHour: vi.fn(),
  resumeSync: vi.fn(),
  runningApps: vi.fn(),
  setAddToHistory: vi.fn(),
  setDeviceSettings: vi.fn(),
  setHistoryPrefs: vi.fn(),
  setPrivacy: vi.fn(),
}));
vi.mock("../../platform/platform", () => ({
  platform: "linux",
  detectPlatform: vi.fn(() => "linux"),
  applyPlatform: vi.fn(),
  modKey: vi.fn(() => "Ctrl"),
  keyStoreName: vi.fn(() => "the Secret Service (KWallet)"),
}));

const getSettings = vi.mocked(commands.getSettings);
const setAddToHistory = vi.mocked(commands.setAddToHistory);

const settingsView: SettingsView = {
  deviceId: "00000000-0000-4000-8000-000000000001",
  deviceName: "cachy-desktop",
  thisDevice: { mode: "both", effectiveMode: "both", ttlSecs: null, pausedUntil: null },
  ttlChoices: [3600, null],
  addToHistory: true,
  skipSensitive: true,
  excludedApps: [{ id: "org.example.private-app", name: "Private App" }],
  builtinExcluded: [],
  historyMaxItems: 500,
  historyKeepDays: 30,
  icloudBridge: false,
  setupDone: true,
};

beforeEach(() => {
  getSettings.mockReset();
  setAddToHistory.mockReset();
  getSettings.mockResolvedValue(settingsView);
  setAddToHistory.mockResolvedValue(undefined);
});

afterEach(cleanup);

async function rendered() {
  render(<ClipboardPage />);
  await screen.findByText("This device");
}

describe("ClipboardPage on Linux", () => {
  it("replaces the excluded-apps picker with the Plasma hint", async () => {
    await rendered();

    expect(
      screen.getByText(/Not available on Plasma: Wayland doesn't tell Pasta which app copied/),
    ).toBeInstanceOf(HTMLElement);
    // No picker, and no rows for the excluded apps it would manage.
    expect(screen.queryByRole("button", { name: "Add app…" })).toBeNull();
    expect(screen.queryByText("Private App")).toBeNull();
  });

  it("skips passwords with the Linux hint instead of the app list", async () => {
    await rendered();

    expect(screen.getByText(/password managers such as KeePassXC/)).toBeInstanceOf(HTMLElement);
  });

  it("offers the Klipper history toggle and the expiry caveat while it is on", async () => {
    const user = userEvent.setup();
    await rendered();

    // The toggle follows the settings; turning it off also changes what expiry means.
    const toggle = screen.getByRole("switch", { name: "Add received items to Klipper" });
    expect(toggle.getAttribute("aria-checked")).toBe("true");
    expect(screen.getByText(/Klipper can't remove a single entry/)).toBeInstanceOf(HTMLElement);
    expect(
      screen.getByText("Clears every received item from the clipboard at once."),
    ).toBeInstanceOf(HTMLElement);

    await user.click(toggle);
    await waitFor(() => expect(setAddToHistory).toHaveBeenCalledExactlyOnceWith(false));
  });

  it("shows the plain expiry hint when received items stay out of Klipper", async () => {
    getSettings.mockResolvedValue({ ...settingsView, addToHistory: false });
    await rendered();

    expect(
      screen.getByText(/Items from your other devices are removed from Klipper and the clipboard/),
    ).toBeInstanceOf(HTMLElement);
    expect(screen.queryByText(/Klipper can't remove a single entry/)).toBeNull();
  });
});

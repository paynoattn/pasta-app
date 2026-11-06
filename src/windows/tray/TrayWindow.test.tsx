import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as commands from "../../ipc/commands";
import type { StatusView } from "../../ipc/bindings/StatusView";
import type { SettingsView } from "../../ipc/bindings/SettingsView";
import TrayWindow from "./TrayWindow";

vi.mock("../../ipc/commands", () => ({
  clearFromHistory: vi.fn(),
  EVENTS: {
    status: "status-changed",
    settings: "settings-changed",
    trayOpened: "tray-opened",
    history: "history-changed",
    expiry: "expiry-changed",
  },
  getSettings: vi.fn(),
  getStatus: vi.fn(),
  historyCopy: vi.fn(),
  historyList: vi.fn(),
  on: vi.fn().mockResolvedValue(() => {}),
  openWindow: vi.fn(),
  pauseForHour: vi.fn(),
  quitApp: vi.fn(),
  resumeSync: vi.fn(),
  retryKeychain: vi.fn(),
  setDeviceSettings: vi.fn(),
}));
vi.mock("../../platform/platform", () => ({
  platform: "macos",
  detectPlatform: vi.fn(() => "macos"),
  applyPlatform: vi.fn(),
  modKey: vi.fn(() => "⌘"),
  keyStoreName: vi.fn(() => "your keychain"),
}));

const getStatus = vi.mocked(commands.getStatus);
const getSettings = vi.mocked(commands.getSettings);
const historyList = vi.mocked(commands.historyList);
const historyCopy = vi.mocked(commands.historyCopy);
const pauseForHour = vi.mocked(commands.pauseForHour);
const resumeSync = vi.mocked(commands.resumeSync);
const openWindow = vi.mocked(commands.openWindow);

function status(overrides: Partial<Awaited<ReturnType<typeof getStatus>>> = {}): StatusView {
  return {
    status: "Connected · 2 devices",
    level: "ok",
    warnings: [],
    thisDevice: { mode: "both", effectiveMode: "both", ttlSecs: null, pausedUntil: null },
    connected: 2,
    retry: false,
    ...overrides,
  };
}

const settings: SettingsView = {
  deviceId: "00000000-0000-4000-8000-000000000001",
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

beforeEach(() => {
  getStatus.mockReset();
  getSettings.mockReset();
  historyList.mockReset();
  historyCopy.mockReset();
  pauseForHour.mockReset();
  resumeSync.mockReset();
  openWindow.mockReset();
  getStatus.mockResolvedValue(status());
  getSettings.mockResolvedValue(settings);
  historyList.mockResolvedValue([]);
  historyCopy.mockResolvedValue(undefined);
});

afterEach(cleanup);

async function rendered() {
  render(<TrayWindow />);
  await screen.findByText("Pause for 1 hour");
}

describe("TrayWindow", () => {
  it("pauses for an hour when the pause button is clicked", async () => {
    const user = userEvent.setup();
    pauseForHour.mockResolvedValue(undefined);
    await rendered();

    await user.click(screen.getByRole("button", { name: /Pause for 1 hour/ }));
    await waitFor(() => expect(pauseForHour).toHaveBeenCalledExactlyOnceWith());
    expect(resumeSync).not.toHaveBeenCalled();
  });

  it("resumes instead when the device is paused", async () => {
    const user = userEvent.setup();
    getStatus.mockResolvedValue(
      status({
        status: "Paused",
        thisDevice: {
          mode: "both",
          effectiveMode: "paused",
          ttlSecs: null,
          pausedUntil: Date.now() + 3_600_000,
        },
      }),
    );
    resumeSync.mockResolvedValue(undefined);
    await rendered();

    await user.click(screen.getByRole("button", { name: "Resume syncing" }));
    await waitFor(() => expect(resumeSync).toHaveBeenCalledExactlyOnceWith());
    expect(pauseForHour).not.toHaveBeenCalled();
  });

  it("copies a recent item again on click", async () => {
    const user = userEvent.setup();
    historyList.mockResolvedValue([
      {
        id: "00000000-0000-4000-8000-000000000003",
        kind: "text",
        preview: "fixture tray text",
        html: null,
        truncated: false,
        bytes: 17,
        sourceApp: "Finder",
        sourceAppId: null,
        originDevice: "win-workstation",
        received: true,
        createdAt: Date.now() - 60_000,
        expiresAt: null,
      },
    ]);
    await rendered();

    await user.click(await screen.findByRole("button", { name: /fixture tray text/ }));
    await waitFor(() =>
      expect(historyCopy).toHaveBeenCalledExactlyOnceWith(
        "00000000-0000-4000-8000-000000000003",
        false,
      ),
    );
  });

  it("opens the settings window with the add-device section from the tray", async () => {
    const user = userEvent.setup();
    openWindow.mockResolvedValue(undefined);
    await rendered();

    await user.click(screen.getByRole("button", { name: "Add Device…" }));
    await waitFor(() =>
      expect(openWindow).toHaveBeenCalledExactlyOnceWith("settings", "add-device"),
    );
  });
});

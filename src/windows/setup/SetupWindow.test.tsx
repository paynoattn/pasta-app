import { cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as commands from "../../ipc/commands";
import type { SetupView } from "../../ipc/bindings/SetupView";
import SetupWindow from "./SetupWindow";

vi.mock("../../ipc/commands", () => ({
  EVENTS: {
    chain: "chain-changed",
    history: "history-changed",
    peers: "peers-changed",
    settings: "settings-changed",
    status: "status-changed",
    update: "update-status",
  },
  on: vi.fn().mockResolvedValue(() => {}),
  finishSetup: vi.fn(),
  openSettingsLink: vi.fn(),
  requestAccessibility: vi.fn(),
  setIcloudBridge: vi.fn(),
  setOpenAtLogin: vi.fn(),
  setUpdatePrefs: vi.fn(),
  setupStatus: vi.fn(),
  shortcutsStatus: vi.fn(),
  startupStatus: vi.fn(),
}));
vi.mock("../../platform/platform", () => ({
  platform: "linux",
  detectPlatform: vi.fn(() => "linux"),
  applyPlatform: vi.fn(),
  modKey: vi.fn(() => "Ctrl"),
  keyStoreName: vi.fn(() => "the Secret Service (KWallet)"),
}));

const setupStatus = vi.mocked(commands.setupStatus);
const startupStatus = vi.mocked(commands.startupStatus);

/** A Linux setup view; only what each test varies is passed in. */
function setupView(overrides: {
  tailscale?: Partial<SetupView["tailscale"]>;
  clipboardHistory?: boolean | null;
  clipboardUnsupported?: string | null;
}): SetupView {
  return {
    os: "linux",
    tailscale: {
      state: "running",
      detail: null,
      address: "100.101.102.103",
      cliOnly: false,
      ...overrides.tailscale,
    },
    chain: "member",
    accessibility: null,
    clipboardHistory: "clipboardHistory" in overrides ? (overrides.clipboardHistory ?? null) : true,
    cloudSync: null,
    clipboardUnsupported: overrides.clipboardUnsupported ?? null,
    openAtLogin: true,
    autoUpdate: false,
    icloudBridge: false,
    panelShortcut: null,
    setupDone: false,
  };
}

beforeEach(() => {
  setupStatus.mockReset();
  startupStatus.mockReset();
  setupStatus.mockResolvedValue(setupView({}));
  startupStatus.mockResolvedValue({
    openAtLogin: true,
    autoUpdate: false,
    checkHours: 24,
    updaterConfigured: true,
    updateStatus: null,
    availableVersion: null,
  });
});

afterEach(cleanup);

async function rendered() {
  render(<SetupWindow />);
  await screen.findByRole("heading", { name: "Welcome to Pasta" });
}

const STEP_HEADINGS = [
  "Welcome to Pasta",
  "Connect Tailscale",
  "Create or join a chain",
  "Permissions & startup",
  "All set",
];

/** Steps through the flow to step `step` (0-based) with its Continue buttons. */
async function stepTo(user: ReturnType<typeof userEvent.setup>, step: number) {
  for (let current = 0; current < step; current++) {
    await user.click(screen.getByRole("button", { name: "Continue" }));
    await screen.findByRole("heading", { name: STEP_HEADINGS[current + 1] });
  }
}

describe("SetupWindow on Linux", () => {
  it("shows the operator hint when tailscaled denied access", async () => {
    const user = userEvent.setup();
    setupStatus.mockResolvedValue(
      setupView({ tailscale: { state: "stopped", detail: "the daemon denied the request" } }),
    );
    await rendered();
    await stepTo(user, 1);

    expect(screen.getByText("Tailscale won't answer Pasta.")).toBeInstanceOf(HTMLElement);
    expect(screen.getByText(/sudo tailscale set --operator=\$USER/)).toBeInstanceOf(HTMLElement);
  });

  it("hides the operator hint when Tailscale failed for another reason", async () => {
    const user = userEvent.setup();
    await rendered();
    await stepTo(user, 1);

    // Running Tailscale, or a failure without a denial in its words.
    expect(screen.queryByText("Tailscale won't answer Pasta.")).toBeNull();

    setupStatus.mockResolvedValue(
      setupView({ tailscale: { state: "stopped", detail: "NeedsLogin" } }),
    );
    cleanup();
    await rendered();
    await stepTo(user, 1);

    expect(screen.queryByText("Tailscale won't answer Pasta.")).toBeNull();
    expect(screen.getByText("Sign in to Tailscale.")).toBeInstanceOf(HTMLElement);
  });

  it("shows the Klipper row and its fix when Klipper isn't running", async () => {
    const user = userEvent.setup();
    setupStatus.mockResolvedValue(
      setupView({
        clipboardHistory: false,
        clipboardUnsupported: "an X11 session instead of Wayland",
      }),
    );
    await rendered();
    await stepTo(user, 3);

    const plasma = screen.getByRole("heading", { name: "KDE Plasma" }).closest("section");
    if (!plasma) throw new Error("the KDE Plasma section is missing");
    expect(within(plasma).getByText(/Add the Clipboard widget to the system tray/)).toBeInstanceOf(
      HTMLElement,
    );
    expect(within(plasma).getByText("Not running")).toBeInstanceOf(HTMLElement);
    expect(within(plasma).getByText("Pasta can't sync on this desktop.")).toBeInstanceOf(
      HTMLElement,
    );
    expect(within(plasma).getByText("Keep Pasta in the system tray")).toBeInstanceOf(HTMLElement);
  });

  it("shows Klipper as running once its history is back", async () => {
    const user = userEvent.setup();
    await rendered();
    await stepTo(user, 3);

    const plasma = screen.getByRole("heading", { name: "KDE Plasma" }).closest("section");
    if (!plasma) throw new Error("the KDE Plasma section is missing");
    expect(within(plasma).getByText("✓ Running")).toBeInstanceOf(HTMLElement);
    expect(within(plasma).queryByText("Pasta can't sync on this desktop.")).toBeNull();
  });

  it("hides the Klipper row on a desktop it can't sync to, keeping its banner", async () => {
    const user = userEvent.setup();
    setupStatus.mockResolvedValue(
      setupView({
        clipboardHistory: null,
        clipboardUnsupported: "an X11 session instead of Wayland",
      }),
    );
    await rendered();
    await stepTo(user, 3);

    const plasma = screen.getByRole("heading", { name: "KDE Plasma" }).closest("section");
    if (!plasma) throw new Error("the KDE Plasma section is missing");
    expect(within(plasma).queryByText("Klipper")).toBeNull();
    expect(within(plasma).queryByText("✓ Running")).toBeNull();
    expect(within(plasma).queryByText("Not running")).toBeNull();
    expect(within(plasma).getByText("Pasta can't sync on this desktop.")).toBeInstanceOf(
      HTMLElement,
    );
    expect(within(plasma).getByText("Keep Pasta in the system tray")).toBeInstanceOf(HTMLElement);
  });

  it("shows the Meta + V tip on the Done step only when Klipper's history is running", async () => {
    const user = userEvent.setup();
    await rendered();
    await stepTo(user, 4);
    expect(
      screen.getByText("Press Meta + V to see items from your other devices in Klipper."),
    ).toBeInstanceOf(HTMLElement);

    setupStatus.mockResolvedValue(setupView({ clipboardHistory: false }));
    cleanup();
    await rendered();
    await stepTo(user, 4);
    expect(
      screen.queryByText("Press Meta + V to see items from your other devices in Klipper."),
    ).toBeNull();

    setupStatus.mockResolvedValue(setupView({ clipboardHistory: null }));
    cleanup();
    await rendered();
    await stepTo(user, 4);
    expect(
      screen.queryByText("Press Meta + V to see items from your other devices in Klipper."),
    ).toBeNull();
  });
});

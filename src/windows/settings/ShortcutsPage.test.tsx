import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as commands from "../../ipc/commands";
import { ShortcutsPage } from "./ShortcutsPage";

const platformMock = vi.hoisted(() => ({ current: "linux" }));

vi.mock("../../ipc/commands", () => ({
  on: vi.fn().mockResolvedValue(() => {}),
  setPanelShortcut: vi.fn(),
  shortcutsStatus: vi.fn(),
  suspendShortcut: vi.fn(),
}));
// The component reads `platform` once at module scope, so the mock's property
// must be a getter for `platformMock.current` to be picked up after import.
vi.mock("../../platform/platform", () => ({
  get platform() {
    return platformMock.current;
  },
  detectPlatform: vi.fn(() => platformMock.current),
  applyPlatform: vi.fn(),
  modKey: vi.fn(() => "Ctrl"),
  keyStoreName: vi.fn(() => "the Secret Service (KWallet)"),
}));

const shortcutsStatus = vi.mocked(commands.shortcutsStatus);
const setPanelShortcut = vi.mocked(commands.setPanelShortcut);
const suspendShortcut = vi.mocked(commands.suspendShortcut);

beforeEach(() => {
  shortcutsStatus.mockReset();
  setPanelShortcut.mockReset();
  suspendShortcut.mockReset();
});

afterEach(cleanup);

describe("ShortcutsPage unsupported", () => {
  it("shows the Win + V hint on Windows", async () => {
    platformMock.current = "windows";
    shortcutsStatus.mockResolvedValue({
      supported: false,
      panel: null,
      defaultPanel: null,
      error: null,
      warning: null,
    });

    render(<ShortcutsPage />);

    expect(await screen.findByText("History panel")).toBeInstanceOf(HTMLElement);
    expect(screen.getByText(/Win \+ V for now/)).toBeInstanceOf(HTMLElement);
    expect(screen.getByText("Open the history panel")).toBeInstanceOf(HTMLElement);
    expect(screen.getByText("Not set")).toBeInstanceOf(HTMLElement);
    expect(setPanelShortcut).not.toHaveBeenCalled();
  });

  it("shows the Klipper hint on Linux", async () => {
    platformMock.current = "linux";
    shortcutsStatus.mockResolvedValue({
      supported: false,
      panel: null,
      defaultPanel: null,
      error: null,
      warning: null,
    });

    render(<ShortcutsPage />);

    expect(await screen.findByText(/Klipper's shortcut for now/)).toBeInstanceOf(HTMLElement);
  });
});

describe("ShortcutsPage supported (macOS)", () => {
  beforeEach(() => {
    platformMock.current = "macos";
    shortcutsStatus.mockResolvedValue({
      supported: true,
      panel: "Shift+Command+V",
      defaultPanel: "Shift+Command+V",
      error: null,
      warning: null,
    });
  });

  it("shows the live accelerator", async () => {
    render(<ShortcutsPage />);

    expect(await screen.findByText("⇧⌘V")).toBeInstanceOf(HTMLElement);
    // The key cap itself is the control; there's no separate Record button.
    expect(
      screen.getByRole("button", { name: "Change the panel shortcut, now ⇧⌘V" }),
    ).toBeInstanceOf(HTMLElement);
    expect(screen.queryByRole("button", { name: "Record" })).toBeNull();
    // At the default, there is nothing to reset.
    expect(screen.queryByRole("button", { name: "Reset" })).toBeNull();
  });

  it("records a new shortcut from a real key press and saves it", async () => {
    const user = userEvent.setup();
    setPanelShortcut.mockImplementation(async (accelerator) => ({
      supported: true,
      panel: accelerator,
      defaultPanel: "Shift+Command+V",
      error: null,
      warning: null,
    }));
    suspendShortcut.mockResolvedValue(undefined);
    render(<ShortcutsPage />);
    await screen.findByText("⇧⌘V");

    await user.click(screen.getByRole("button", { name: /Change the panel shortcut/ }));
    expect(await screen.findByText("Press a shortcut…")).toBeInstanceOf(HTMLElement);
    await waitFor(() => expect(suspendShortcut).toHaveBeenNthCalledWith(1, true));

    await user.keyboard("{Control>}{Alt>}p");
    await waitFor(() => expect(setPanelShortcut).toHaveBeenCalledExactlyOnceWith("Control+Alt+P"));
    // Recording suspended the panel shortcut while listening, then restored it.
    await waitFor(() => expect(suspendShortcut).toHaveBeenLastCalledWith(false));
    // Displayed with macOS glyphs, not the wire format.
    expect(await screen.findByText("⌃⌥P")).toBeInstanceOf(HTMLElement);
    // The new shortcut differs from the default, so Reset appears.
    expect(screen.getByRole("button", { name: "Reset" })).toBeInstanceOf(HTMLElement);
  });
});

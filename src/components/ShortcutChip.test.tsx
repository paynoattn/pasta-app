import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as commands from "../ipc/commands";
import type { ShortcutsView } from "../ipc/commands";
import { useShortcutRecorder } from "../state/useShortcutRecorder";
import { ShortcutChip } from "./ShortcutChip";

vi.mock("../ipc/commands", () => ({
  setPanelShortcut: vi.fn(),
  suspendShortcut: vi.fn(),
}));
vi.mock("../platform/platform", () => ({ platform: "macos" }));

const setPanelShortcut = vi.mocked(commands.setPanelShortcut);
const suspendShortcut = vi.mocked(commands.suspendShortcut);

const DEFAULT: ShortcutsView = {
  supported: true,
  panel: "Shift+Command+V",
  defaultPanel: "Shift+Command+V",
  error: null,
  warning: null,
};

function Harness() {
  const [view, setView] = useState(DEFAULT);
  const recorder = useShortcutRecorder(setView);
  return <ShortcutChip view={view} recorder={recorder} />;
}

beforeEach(() => {
  setPanelShortcut.mockReset();
  suspendShortcut.mockReset();
  suspendShortcut.mockResolvedValue(undefined);
});

afterEach(cleanup);

describe("ShortcutChip", () => {
  it("records a new shortcut when the key cap is clicked", async () => {
    const user = userEvent.setup();
    setPanelShortcut.mockImplementation(async (accelerator) => ({
      ...DEFAULT,
      panel: accelerator,
    }));
    render(<Harness />);

    await user.click(screen.getByRole("button", { name: "Change the panel shortcut, now ⇧⌘V" }));
    expect(screen.getByText("Press a shortcut…")).toBeInstanceOf(HTMLElement);
    await waitFor(() => expect(suspendShortcut).toHaveBeenCalledWith(true));

    await user.keyboard("{Control>}{Alt>}p");
    await waitFor(() => expect(setPanelShortcut).toHaveBeenCalledExactlyOnceWith("Control+Alt+P"));
    expect(await screen.findByText("⌃⌥P")).toBeInstanceOf(HTMLElement);
    await waitFor(() => expect(suspendShortcut).toHaveBeenLastCalledWith(false));
  });

  it("Esc cancels without changing the shortcut", async () => {
    const user = userEvent.setup();
    render(<Harness />);

    await user.click(screen.getByRole("button", { name: /Change the panel shortcut/ }));
    await user.keyboard("{Escape}");

    expect(await screen.findByText("⇧⌘V")).toBeInstanceOf(HTMLElement);
    expect(setPanelShortcut).not.toHaveBeenCalled();
    await waitFor(() => expect(suspendShortcut).toHaveBeenLastCalledWith(false));
  });
});

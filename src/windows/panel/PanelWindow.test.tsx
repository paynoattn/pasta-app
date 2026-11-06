import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import * as commands from "../../ipc/commands";
import type { HistoryItemView } from "../../ipc/bindings/HistoryItemView";
import PanelWindow from "./PanelWindow";

vi.mock("../../ipc/commands", () => ({
  appIcon: vi.fn(),
  EVENTS: { history: "history-changed", panelOpened: "panel-opened" },
  hidePanel: vi.fn(),
  historyDelete: vi.fn(),
  historyList: vi.fn(),
  historyPaste: vi.fn(),
  on: vi.fn().mockResolvedValue(() => {}),
}));

const historyList = vi.mocked(commands.historyList);
const historyPaste = vi.mocked(commands.historyPaste);
const hidePanel = vi.mocked(commands.hidePanel);
const on = vi.mocked(commands.on);

// jsdom has no layout, so scrolling is a no-op there.
beforeAll(() => {
  Element.prototype.scrollIntoView = () => {};
});

function item(id: string, overrides: Partial<HistoryItemView> = {}): HistoryItemView {
  return {
    id,
    kind: "text",
    preview: `preview of ${id}`,
    html: null,
    truncated: false,
    bytes: 42,
    sourceApp: "Finder",
    sourceAppId: null,
    originDevice: null,
    received: false,
    createdAt: Date.now() - 60_000,
    expiresAt: null,
    ...overrides,
  };
}

const FIRST = item("00000000-0000-4000-8000-000000000001");
const SECOND = item("00000000-0000-4000-8000-000000000002");

beforeEach(() => {
  historyList.mockReset();
  historyPaste.mockReset();
  hidePanel.mockReset();
  on.mockClear();
  historyList.mockResolvedValue([FIRST, SECOND]);
  historyPaste.mockResolvedValue("pasted");
});

afterEach(cleanup);

async function rendered() {
  const view = render(<PanelWindow />);
  const strip = await screen.findByRole("listbox", { name: "Clipboard history" });
  expect(screen.getByRole("option", { name: /preview of /, selected: true })).toBeTruthy();
  return { user: userEvent.setup(), strip, view };
}

describe("PanelWindow", () => {
  it("ArrowRight then Enter pastes the second item as formatted text", async () => {
    const { user, strip } = await rendered();
    strip.focus();

    await user.keyboard("{ArrowRight}");
    // The second card is now the active descendant.
    expect(strip.getAttribute("aria-activedescendant")).toBe("card-1");

    await user.keyboard("{Enter}");
    await waitFor(() => expect(historyPaste).toHaveBeenCalledExactlyOnceWith(SECOND.id, false));
  });

  it("Shift+Enter pastes the selected item as plain text", async () => {
    const { user, strip } = await rendered();
    strip.focus();

    await user.keyboard("{Shift>}{Enter}{/Shift}");
    await waitFor(() => expect(historyPaste).toHaveBeenCalledExactlyOnceWith(FIRST.id, true));
  });

  it("Escape hides the panel", async () => {
    const { user, strip } = await rendered();
    strip.focus();

    await user.keyboard("{Escape}");
    await waitFor(() => expect(hidePanel).toHaveBeenCalledExactlyOnceWith());
  });

  it("Backspace deletes the selected card", async () => {
    vi.mocked(commands.historyDelete).mockResolvedValue(undefined);
    const { user, strip } = await rendered();
    strip.focus();

    await user.keyboard("{ArrowRight}{Backspace}");
    await waitFor(() =>
      expect(vi.mocked(commands.historyDelete)).toHaveBeenCalledExactlyOnceWith(SECOND.id),
    );
    expect(historyPaste).not.toHaveBeenCalled();
  });

  it("renders sanitised rich text without scripts", async () => {
    historyList.mockResolvedValue([
      item("00000000-0000-4000-8000-000000000003", {
        preview: "bold fixture text",
        // Obvious fixture payload: the sanitizer must drop the script entirely.
        html: "<b>bold fixture text</b><script>alert('fixture')</script>",
      }),
    ]);
    render(<PanelWindow />);

    const rich = await screen.findByText("bold fixture text");
    expect(rich.tagName).toBe("B");
    const body = rich.closest(".card__body");
    expect(body?.querySelector("script")).toBeNull();
    expect(body?.innerHTML).not.toContain("alert");
  });

  it("shows the paste target from the context", async () => {
    render(<PanelWindow />);
    expect(await screen.findByText("Paste into the app in front")).toBeInstanceOf(HTMLElement);
  });
});

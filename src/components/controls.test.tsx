import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { Dialog } from "./controls";

/** Renders the dialog the way real callers do: a new `onClose` closure every render. */
function TypingDialog() {
  const [text, setText] = useState("");
  return (
    <Dialog open title="Rename" onClose={() => setText("closed")}>
      <label>
        Name
        <input value={text} onChange={(event) => setText(event.target.value)} />
      </label>
    </Dialog>
  );
}

afterEach(cleanup);

describe("Dialog", () => {
  it("focuses the first field when it opens", () => {
    render(
      <Dialog open title="Two fields" onClose={() => {}}>
        <input />
        <input />
      </Dialog>,
    );

    const [first, second] = screen.getAllByRole("textbox");
    expect(document.activeElement).toBe(first);
    expect(document.activeElement).not.toBe(second);
  });

  it("focuses itself when it has no field", () => {
    render(
      <Dialog open title="No fields" onClose={() => {}}>
        <p>Nothing to type here.</p>
      </Dialog>,
    );

    expect(document.activeElement).toBe(screen.getByRole("dialog"));
  });

  it("keeps focus and the typed value while the parent re-renders with a fresh onClose per keystroke", async () => {
    const user = userEvent.setup();
    render(<TypingDialog />);

    // Every keystroke re-renders the parent, so `onClose` is a new closure each
    // time; the dialog must not pull focus back out of the input.
    const input = screen.getByRole("textbox");
    await user.type(input, "pasta");

    expect(document.activeElement).toBe(input);
    expect((input as HTMLInputElement).value).toBe("pasta");
  });

  it("Escape calls the latest onClose", async () => {
    const user = userEvent.setup();
    const stale = vi.fn();
    const latest = vi.fn();
    const { rerender } = render(
      <Dialog open title="Sheet" onClose={stale}>
        <p>Body.</p>
      </Dialog>,
    );
    rerender(
      <Dialog open title="Sheet" onClose={latest}>
        <p>Body.</p>
      </Dialog>,
    );

    await user.keyboard("{Escape}");

    expect(latest).toHaveBeenCalledTimes(1);
    expect(stale).not.toHaveBeenCalled();
  });
});

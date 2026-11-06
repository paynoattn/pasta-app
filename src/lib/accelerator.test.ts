import { describe, expect, it } from "vitest";
import { displayAccelerator, fromKeyboardEvent } from "./accelerator";

function key(partial: Partial<Parameters<typeof fromKeyboardEvent>[0]>) {
  return {
    code: "",
    key: "",
    ctrlKey: false,
    altKey: false,
    shiftKey: false,
    metaKey: false,
    ...partial,
  };
}

describe("fromKeyboardEvent", () => {
  it("builds the accelerator with modifiers in Pasta's order", () => {
    expect(fromKeyboardEvent(key({ code: "KeyV", key: "v", metaKey: true, shiftKey: true }))).toBe(
      "Shift+Command+V",
    );
    expect(fromKeyboardEvent(key({ code: "KeyK", key: "k", ctrlKey: true, altKey: true }))).toBe(
      "Control+Alt+K",
    );
    expect(
      fromKeyboardEvent(
        key({
          code: "F12",
          key: "F12",
          ctrlKey: true,
          altKey: true,
          shiftKey: true,
          metaKey: true,
        }),
      ),
    ).toBe("Control+Alt+Shift+Command+F12");
  });

  it("maps digits, function keys and named keys", () => {
    expect(fromKeyboardEvent(key({ code: "Digit1", key: "1", metaKey: true }))).toBe("Command+1");
    expect(fromKeyboardEvent(key({ code: "F5", key: "F5", metaKey: true }))).toBe("Command+F5");
    expect(fromKeyboardEvent(key({ code: "ArrowLeft", key: "ArrowLeft", metaKey: true }))).toBe(
      "Command+Left",
    );
    expect(fromKeyboardEvent(key({ code: "Space", key: " ", metaKey: true }))).toBe(
      "Command+Space",
    );
    expect(fromKeyboardEvent(key({ code: "Enter", key: "Enter", metaKey: true }))).toBe(
      "Command+Enter",
    );
  });

  it("ignores modifiers alone and unknown keys", () => {
    expect(fromKeyboardEvent(key({ code: "ShiftLeft", key: "Shift" }))).toBeNull();
    expect(fromKeyboardEvent(key({ code: "MetaLeft", key: "Meta", shiftKey: true }))).toBeNull();
    expect(
      fromKeyboardEvent(key({ code: "AudioVolumeUp", key: "AudioVolumeUp", metaKey: true })),
    ).toBeNull();
  });

  it("records a bare letter while a shortcut is being typed", () => {
    // No modifiers: still a key event, so the recorder sees it (Rust later refuses bare keys).
    expect(fromKeyboardEvent(key({ code: "KeyV", key: "v" }))).toBe("V");
  });
});

describe("displayAccelerator", () => {
  it("renders glyphs for macOS and keeps unknown parts", () => {
    expect(displayAccelerator("Shift+Command+V")).toBe("⇧⌘V");
    expect(displayAccelerator("Control+Alt+K")).toBe("⌃⌥K");
    expect(displayAccelerator("Command+Enter")).toBe("⌘↩");
    expect(displayAccelerator("Escape")).toBe("⎋");
    expect(displayAccelerator("Command+Space")).toBe("⌘Space");
    expect(displayAccelerator("Shift+Command+F5")).toBe("⇧⌘F5");
  });
});

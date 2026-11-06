/**
 * Recording shortcuts (#48). Produces Pasta's accelerator form, e.g.
 * `Shift+Command+V`; Rust validates and registers it.
 */
const MODIFIER_KEYS = new Set(["Shift", "Control", "Alt", "Meta", "OS", "Hyper", "Super"]);

const NAMED: Record<string, string> = {
  Space: "Space",
  Enter: "Enter",
  Tab: "Tab",
  Escape: "Escape",
  Backspace: "Backspace",
  Delete: "Delete",
  ArrowUp: "Up",
  ArrowDown: "Down",
  ArrowLeft: "Left",
  ArrowRight: "Right",
  Comma: "Comma",
  Period: "Period",
  Slash: "Slash",
  Semicolon: "Semicolon",
  Quote: "Quote",
  Backquote: "Backquote",
  Minus: "Minus",
  Equal: "Equal",
  BracketLeft: "BracketLeft",
  BracketRight: "BracketRight",
  Backslash: "Backslash",
};

/** The key part of an accelerator from `KeyboardEvent.code`, or `null` for modifiers and unknown keys. */
function keyName(code: string, key: string): string | null {
  if (MODIFIER_KEYS.has(key)) return null;
  if (/^Key[A-Z]$/.test(code)) return code.slice(3);
  if (/^Digit[0-9]$/.test(code)) return code.slice(5);
  if (/^F([1-9]|1[0-9]|20)$/.test(code)) return code;
  return NAMED[code] ?? null;
}

/** The accelerator a key press makes, or `null` while only modifiers are held. */
export function fromKeyboardEvent(event: {
  code: string;
  key: string;
  ctrlKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
  metaKey: boolean;
}): string | null {
  const key = keyName(event.code, event.key);
  if (!key) return null;
  const parts: string[] = [];
  if (event.ctrlKey) parts.push("Control");
  if (event.altKey) parts.push("Alt");
  if (event.shiftKey) parts.push("Shift");
  if (event.metaKey) parts.push("Command");
  parts.push(key);
  return parts.join("+");
}

const GLYPHS: Record<string, string> = {
  Control: "⌃",
  Alt: "⌥",
  Shift: "⇧",
  Command: "⌘",
  Enter: "↩",
  Escape: "⎋",
  Backspace: "⌫",
  Delete: "⌦",
  Up: "↑",
  Down: "↓",
  Left: "←",
  Right: "→",
  Tab: "⇥",
  Space: "Space",
};

/** `Shift+Command+V` → `⇧⌘V`. */
export function displayAccelerator(accelerator: string): string {
  return accelerator
    .split("+")
    .map((part) => GLYPHS[part] ?? part)
    .join("");
}

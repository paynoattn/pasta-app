import { describe, expect, it } from "vitest";
import { sections, searchSections } from "./sections";

describe("sections", () => {
  it("has the six sections in order", () => {
    const all = sections("macos");
    expect(all.map((section) => section.id)).toEqual([
      "devices",
      "clipboard",
      "chain",
      "shortcuts",
      "startup",
      "about",
    ]);
  });

  it("adapts the per-OS settings", () => {
    const mac = sections("macos");
    expect(mac[0].settings).toContain("Bridge iCloud Universal Clipboard");
    expect(mac[1].settings).toContain("History size");
    expect(mac[1].settings).not.toContain("Add received items to Win + V");
    expect(mac[3].settings).toContain("Open the clipboard panel");

    const windows = sections("windows");
    expect(windows[0].settings).not.toContain("Bridge iCloud Universal Clipboard");
    expect(windows[1].settings).toContain("Add received items to Win + V");
    expect(windows[3].settings).toContain("History panel shortcut");

    const linux = sections("linux");
    expect(linux[1].settings).not.toContain("Clear from Win + V");
    expect(linux[3].settings).toContain("History panel shortcut");
  });
});

describe("searchSections", () => {
  const all = sections("macos");

  it("returns everything with no matches for an empty query", () => {
    const found = searchSections(all, "   ");
    expect(found).toHaveLength(all.length);
    expect(found.every(({ matches }) => matches.length === 0)).toBe(true);
  });

  it("matches settings case-insensitively and returns the matching settings", () => {
    const found = searchSections(all, "expire");
    expect(found.map(({ section }) => section.id)).toEqual(["clipboard"]);
    const clipboard = found[0];
    expect(clipboard.matches).toContain("Expire received items after");
    expect(clipboard.matches).toContain("Expire all now");
  });

  it("matches a section by its own wording, not just the query's case", () => {
    const found = searchSections(all, "EXPIRY");
    expect(found.map(({ section }) => section.id)).toEqual(["devices", "clipboard"]);
    expect(found[0].matches).toEqual(["Expiry per device"]);
  });

  it("keeps a section whose title matches even without a setting match", () => {
    const found = searchSections(all, "about");
    expect(found).toHaveLength(1);
    expect(found[0].section.id).toBe("about");
    expect(found[0].matches).toHaveLength(0);
  });

  it("drops sections with nothing matching", () => {
    expect(searchSections(all, "zzz-not-a-setting")).toEqual([]);
  });

  it("finds settings by one word of a longer phrase", () => {
    const found = searchSections(all, "icloud");
    expect(found.map(({ section }) => section.id)).toEqual(["devices"]);
  });
});

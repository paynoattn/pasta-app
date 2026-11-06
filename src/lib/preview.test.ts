import { describe, expect, it } from "vitest";
import { appHue, hasFormatting, hostname, linkTitle, sanitizeHtml } from "./preview";

describe("sanitizeHtml", () => {
  it("strips scripts, styles, images, frames and forms but keeps formatting", () => {
    const dirty = [
      "<p>hello <b>world</b></p>",
      "<script>alert('x')</script>",
      "<style>body{}</style>",
      "<img src=https://evil.example/pixel>",
      "<iframe src=https://evil.example></iframe>",
      "<form action=https://evil.example><input value=secret></form>",
    ].join("");
    const clean = sanitizeHtml(dirty);
    expect(clean).toContain("<b>world</b>");
    expect(clean).not.toContain("script");
    expect(clean).not.toContain("style");
    expect(clean).not.toContain("<img");
    expect(clean).not.toContain("iframe");
    expect(clean).not.toContain("form");
    expect(clean).not.toContain("secret");
  });

  it("drops every attribute: handlers, style, href, class", () => {
    const clean = sanitizeHtml(
      '<a href="https://evil.example" style="color:red" class="fancy" onclick="steal()">a link</a>' +
        '<div onmouseover="steal()" style="background:url(https://evil.example)">text</div>',
    );
    expect(clean).not.toContain("href");
    expect(clean).not.toContain("onclick");
    expect(clean).not.toContain("onmouseover");
    expect(clean).not.toContain("style");
    expect(clean).not.toContain("class");
    expect(clean).toContain("a link");
    expect(clean).toContain("text");
  });

  it("keeps list and table structure", () => {
    const clean = sanitizeHtml(
      "<ul><li>one</li><li>two</li></ul><table><tr><td>x</td></tr></table>",
    );
    expect(clean).toContain("<li>one</li>");
    expect(clean).toContain("<td>x</td>");
  });
});

describe("hasFormatting", () => {
  it("spots real formatting and ignores bare containers", () => {
    expect(hasFormatting("<b>bold</b>")).toBe(true);
    expect(hasFormatting("<li>item</li>")).toBe(true);
    expect(hasFormatting("<h2>Title</h2>")).toBe(true);
    expect(hasFormatting("<p>plain paragraph</p>")).toBe(false);
    expect(hasFormatting("<div>plain div</div>")).toBe(false);
    expect(hasFormatting("just text")).toBe(false);
  });
});

describe("hostname", () => {
  it("parses with or without a scheme and strips www", () => {
    expect(hostname("https://example.com/path")).toBe("example.com");
    expect(hostname("example.com")).toBe("example.com");
    expect(hostname("http://www.example.com/x?y=1")).toBe("example.com");
    expect(hostname("https://blog.example.com")).toBe("blog.example.com");
  });

  it("returns null for non-URLs", () => {
    expect(hostname("not a url")).toBeNull();
    expect(hostname("   ")).toBeNull();
    expect(hostname("")).toBeNull();
  });
});

describe("linkTitle", () => {
  it("uses anchor text that isn't the URL itself", () => {
    expect(linkTitle('<a href="https://example.com">Quarterly report</a>')).toBe(
      "Quarterly report",
    );
    expect(linkTitle('<a href="https://example.com">https://example.com</a>')).toBeNull();
    expect(linkTitle(null)).toBeNull();
    expect(linkTitle("<p>no anchor</p>")).toBeNull();
  });

  it("ignores text from markup that would be stripped anyway", () => {
    expect(linkTitle('<script>alert(1)</script><a href="https://ok.example">Docs</a>')).toBe(
      "Docs",
    );
  });
});

describe("appHue", () => {
  it("is stable per app and defaults for null", () => {
    expect(appHue(null)).toBe(210);
    const notes = appHue("com.apple.Notes");
    expect(notes).toBeGreaterThanOrEqual(0);
    expect(notes).toBeLessThan(360);
    expect(appHue("com.apple.Notes")).toBe(notes);
    expect(appHue("com.apple.Notes ")).not.toBe(notes);
    // Different apps usually differ; at least it stays in range.
    const other = appHue("com.apple.Safari");
    expect(other).toBeGreaterThanOrEqual(0);
    expect(other).toBeLessThan(360);
  });
});

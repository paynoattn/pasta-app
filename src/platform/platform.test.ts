import { describe, expect, it } from "vitest";
import { applyPlatform, detectPlatform, keyStoreName, modKey } from "./platform";

describe("detectPlatform", () => {
  it("recognises the three OS families from a user agent", () => {
    expect(
      detectPlatform(
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/620.1.15 (KHTML, like Gecko)",
      ),
    ).toBe("macos");
    expect(detectPlatform("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")).toBe(
      "windows",
    );
    // jsdom's own agent says "jsdom", which falls through to linux.
    expect(detectPlatform("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36")).toBe("linux");
    expect(detectPlatform("jsdom")).toBe("linux");
  });
});

describe("platform helpers", () => {
  it("label the modifier and key store per OS", () => {
    expect(modKey("macos")).toBe("⌘");
    expect(modKey("windows")).toBe("Ctrl");
    expect(modKey("linux")).toBe("Ctrl");
    expect(keyStoreName("macos")).toBe("the macOS Keychain");
    expect(keyStoreName("windows")).toBe("Windows Credential Manager");
    expect(keyStoreName("linux")).toBe("the Secret Service (KWallet)");
  });

  it("applyPlatform sets data-platform on the root", () => {
    const root = document.createElement("html");
    applyPlatform(root, "windows");
    expect(root.dataset.platform).toBe("windows");
    applyPlatform(root);
    expect(root.dataset.platform).toBe("linux"); // jsdom's agent
  });
});

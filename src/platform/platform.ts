/** The OS look Pasta follows (#28): macOS, Windows 11 Fluent, or KDE Breeze. */
export type Platform = "macos" | "windows" | "linux";

/** Guessed from the webview's user agent, so the first paint already has the right theme. */
export function detectPlatform(userAgent: string = navigator.userAgent): Platform {
  if (/Mac OS X|Macintosh/i.test(userAgent)) return "macos";
  if (/Windows/i.test(userAgent)) return "windows";
  return "linux";
}

export const platform: Platform = detectPlatform();

/** Sets `data-platform` on the root element; the theme tokens key off it. */
export function applyPlatform(root: HTMLElement = document.documentElement, os = platform): void {
  root.dataset.platform = os;
}

/** The modifier key's symbol or name, for hints. */
export function modKey(os: Platform = platform): string {
  return os === "macos" ? "⌘" : "Ctrl";
}

/** Where this OS keeps Pasta's keys (#23). */
export function keyStoreName(os: Platform = platform): string {
  switch (os) {
    case "macos":
      return "the macOS Keychain";
    case "windows":
      return "Windows Credential Manager";
    default:
      return "the Secret Service (KWallet)";
  }
}

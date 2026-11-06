import type { Platform } from "../../platform/platform";

export type SectionId = "devices" | "clipboard" | "chain" | "shortcuts" | "startup" | "about";

export interface SectionInfo {
  id: SectionId;
  title: string;
  /** Settings on the page, for search (#29). */
  settings: string[];
}

export function sections(os: Platform): SectionInfo[] {
  return [
    {
      id: "devices",
      title: "Devices",
      settings: [
        "Add device",
        "Approve or decline a device",
        "Send, receive, both or paused",
        "Expiry per device",
        "Rename this device",
        "Remove device",
        "Latency and tailnet address",
        ...(os === "macos" ? ["Bridge iCloud Universal Clipboard"] : []),
      ],
    },
    {
      id: "clipboard",
      title: "Clipboard & expiry",
      settings: [
        "Mode of this device",
        "Pause for 1 hour",
        "Expire received items after",
        "Expire all now",
        ...(os === "windows" ? ["Add received items to Win + V", "Clear from Win + V"] : []),
        ...(os === "macos" ? ["History size", "Keep history for", "Clear history"] : []),
        "Skip passwords and other sensitive items",
        "Excluded apps",
      ],
    },
    {
      id: "chain",
      title: "Sync chain & security",
      settings: [
        "Show the 24 words",
        "QR code",
        "Copy words",
        "Where the keys are kept",
        "End-to-end encryption",
        "Leave the chain",
      ],
    },
    {
      id: "shortcuts",
      title: "Shortcuts",
      settings:
        os === "macos"
          ? ["Open the clipboard panel", "Record a shortcut"]
          : ["History panel shortcut"],
    },
    {
      id: "startup",
      title: "Startup & updates",
      settings: ["Open at login", "Update automatically", "Check for updates"],
    },
    {
      id: "about",
      title: "About",
      settings: [
        "Version",
        "Diagnostics",
        "Copy diagnostics",
        "Open log folder",
        "Run setup again",
      ],
    },
  ];
}

/** Sections whose title or settings match `query`, with the matching settings. */
export function searchSections(
  all: SectionInfo[],
  query: string,
): { section: SectionInfo; matches: string[] }[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return all.map((section) => ({ section, matches: [] }));
  return all
    .map((section) => ({
      section,
      matches: section.settings.filter((setting) => setting.toLowerCase().includes(needle)),
    }))
    .filter(
      ({ section, matches }) => matches.length > 0 || section.title.toLowerCase().includes(needle),
    );
}

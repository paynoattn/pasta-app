import { describe, expect, it } from "vitest";
import {
  MODE_HINTS,
  MODE_LABELS,
  countdown,
  formatBytes,
  formatTtl,
  formatTtlShort,
  osLabel,
  parseTtl,
  pausedFor,
  timeAgo,
  ttlOptions,
  ttlValue,
} from "./format";

describe("formatTtl", () => {
  it("formats whole units and special cases", () => {
    expect(formatTtl(null)).toBe("Never");
    expect(formatTtl(5)).toBe("5 seconds");
    expect(formatTtl(60)).toBe("1 minute");
    expect(formatTtl(300)).toBe("5 minutes");
    expect(formatTtl(3600)).toBe("1 hour");
    expect(formatTtl(86_400)).toBe("1 day");
    expect(formatTtl(604_800)).toBe("7 days");
    expect(formatTtl(1_209_600)).toBe("2 weeks");
  });

  it("counts leftover seconds rather than rounding up a unit", () => {
    expect(formatTtl(61)).toBe("61 seconds");
    expect(formatTtl(90)).toBe("90 seconds");
  });
});

describe("formatTtlShort", () => {
  it("compacts the same values for tight spots", () => {
    expect(formatTtlShort(null)).toBe("Never");
    expect(formatTtlShort(45)).toBe("45s");
    expect(formatTtlShort(300)).toBe("5m");
    expect(formatTtlShort(7_200)).toBe("2h");
    expect(formatTtlShort(3 * 86_400)).toBe("3d");
  });
});

describe("countdown", () => {
  it("labels kept items, due items and the remaining time", () => {
    const now = 1_000_000;
    expect(countdown(null, now)).toBe("kept");
    expect(countdown(now - 1, now)).toBe("expiring…");
    expect(countdown(now, now)).toBe("expiring…");
    expect(countdown(now + 45_000, now)).toBe("expires in 45 s");
    expect(countdown(now + 13 * 60_000, now)).toBe("expires in 13 min");
    expect(countdown(now + 2 * 3_600_000, now)).toBe("expires in 2 h");
    expect(countdown(now + 3 * 86_400_000, now)).toBe("expires in 3 d");
  });

  it("rounds partial minutes up, never down to zero", () => {
    const now = 1_000_000;
    expect(countdown(now + 61_000, now)).toBe("expires in 2 min");
    expect(countdown(now + 1, now)).toBe("expires in 1 s");
  });
});

describe("timeAgo", () => {
  const now = 1_730_000_000_000;
  it("goes from now to a locale date", () => {
    expect(timeAgo(now, now)).toBe("now");
    expect(timeAgo(now - 44_000, now)).toBe("now");
    expect(timeAgo(now - 5 * 60_000, now)).toBe("5 min ago");
    expect(timeAgo(now - 3 * 3_600_000, now)).toBe("3 h ago");
    expect(timeAgo(now - 86_400_000, now)).toBe("yesterday");
    expect(timeAgo(now - 2 * 86_400_000 + 1, now)).not.toBe("yesterday");
  });

  it("shows a short date for anything older", () => {
    const at = new Date(now - 30 * 86_400_000).getTime();
    const expected = new Date(at).toLocaleDateString(undefined, { month: "short", day: "numeric" });
    expect(timeAgo(at, now)).toBe(expected);
  });
});

describe("formatBytes", () => {
  it("uses B, KB and MB", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(12)).toBe("12 B");
    expect(formatBytes(1_228.8)).toBe("1.2 KB");
    expect(formatBytes(50 * 1024)).toBe("50 KB");
    expect(formatBytes((3.4 * 1024 * 1024) | 0)).toBe("3.4 MB");
  });
});

describe("mode labels and hints", () => {
  it("cover every mode", () => {
    expect(Object.keys(MODE_LABELS).sort()).toEqual(["both", "paused", "receive", "send"]);
    expect(MODE_LABELS.both).toBe("Both");
    expect(MODE_HINTS.paused).toMatch(/nothing goes out or comes in/i);
  });
});

describe("osLabel", () => {
  it("names the three platforms and passes anything else through", () => {
    expect(osLabel("macos")).toBe("macOS");
    expect(osLabel("windows")).toBe("Windows");
    expect(osLabel("linux")).toBe("Linux");
    expect(osLabel("haiku")).toBe("haiku");
  });
});

describe("pausedFor", () => {
  it("describes a running pause", () => {
    const now = 1_000_000;
    expect(pausedFor(now + 42 * 60_000, now)).toBe("for 42 more min");
    expect(pausedFor(now + 60 * 60_000, now)).toBe("for 1 hour");
    expect(pausedFor(now + 1, now)).toBe("for 1 more min");
  });
});

describe("ttl drop-down helpers", () => {
  it("maps the choices to values and labels", () => {
    expect(ttlOptions([5, 3600, null])).toEqual([
      { value: "5", label: "5 seconds" },
      { value: "3600", label: "1 hour" },
      { value: "never", label: "Never" },
    ]);
    expect(ttlValue(null)).toBe("never");
    expect(ttlValue(300)).toBe("300");
    expect(parseTtl("never")).toBeNull();
    expect(parseTtl("300")).toBe(300);
  });
});

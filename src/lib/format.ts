import type { Mode } from "../ipc/commands";

/** "5 seconds", "1 hour", "Never". */
export function formatTtl(secs: number | null): string {
  if (secs === null) return "Never";
  const units: [number, string][] = [
    [604_800, "week"],
    [86_400, "day"],
    [3600, "hour"],
    [60, "minute"],
    [1, "second"],
  ];
  for (const [size, name] of units) {
    if (secs >= size && secs % size === 0) {
      const count = secs / size;
      if (name === "week" && count === 1) return "7 days";
      return `${count} ${name}${count === 1 ? "" : "s"}`;
    }
  }
  return `${secs} seconds`;
}

/** "5s", "1h", "Never": for tight spots like the tray. */
export function formatTtlShort(secs: number | null): string {
  if (secs === null) return "Never";
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.round(secs / 60)}m`;
  if (secs < 86_400) return `${Math.round(secs / 3600)}h`;
  return `${Math.round(secs / 86_400)}d`;
}

/** "expires in 13 min", "expires in 45 s", "expired". */
export function countdown(expiresAt: number | null, now: number): string {
  if (expiresAt === null) return "kept";
  const left = Math.ceil((expiresAt - now) / 1000);
  if (left <= 0) return "expiring…";
  if (left < 60) return `expires in ${left} s`;
  if (left < 3600) return `expires in ${Math.ceil(left / 60)} min`;
  if (left < 86_400) return `expires in ${Math.round(left / 3600)} h`;
  return `expires in ${Math.round(left / 86_400)} d`;
}

/** "now", "5 min ago", "3 h ago", "yesterday", or a date. */
export function timeAgo(at: number, now: number): string {
  const secs = Math.max(0, Math.round((now - at) / 1000));
  if (secs < 45) return "now";
  if (secs < 3600) return `${Math.max(1, Math.round(secs / 60))} min ago`;
  if (secs < 86_400) return `${Math.round(secs / 3600)} h ago`;
  if (secs < 2 * 86_400) return "yesterday";
  return new Date(at).toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

/** "12 B", "1.2 KB", "3.4 MB". */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(bytes < 10 * 1024 ? 1 : 0)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

export const MODE_LABELS: Record<Mode, string> = {
  send: "Send",
  receive: "Receive",
  both: "Both",
  paused: "Paused",
};

/** What a mode means, for hints. */
export const MODE_HINTS: Record<Mode, string> = {
  send: "Copies here go to your other devices; nothing comes in.",
  receive: "Items from your other devices arrive here; copies here stay here.",
  both: "Copies go out and items come in.",
  paused: "Nothing goes out or comes in.",
};

export function osLabel(os: string): string {
  switch (os) {
    case "macos":
      return "macOS";
    case "windows":
      return "Windows";
    case "linux":
      return "Linux";
    default:
      return os;
  }
}

/** "in 42 min" for a pause that ends at `until`. */
export function pausedFor(until: number, now: number): string {
  const mins = Math.max(1, Math.ceil((until - now) / 60_000));
  return mins >= 60 ? "for 1 hour" : `for ${mins} more min`;
}

/** The expiry choices for a drop-down: values are seconds or `never`. */
export function ttlOptions(choices: (number | null)[]): { value: string; label: string }[] {
  return choices.map((secs) => ({ value: ttlValue(secs), label: formatTtl(secs) }));
}

export function ttlValue(secs: number | null): string {
  return secs === null ? "never" : String(secs);
}

export function parseTtl(value: string): number | null {
  return value === "never" ? null : Number(value);
}

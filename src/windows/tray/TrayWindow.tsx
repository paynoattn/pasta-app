import {
  clearFromHistory,
  EVENTS,
  getSettings,
  getStatus,
  historyCopy,
  historyList,
  openWindow,
  pauseForHour,
  quitApp,
  resumeSync,
  retryKeychain,
  setDeviceSettings,
  type HistoryItemView,
  type Mode,
} from "../../ipc/commands";
import { Button, ErrorText, Segmented, StatusDot } from "../../components/controls";
import { countdown, MODE_LABELS, pausedFor, timeAgo } from "../../lib/format";
import { platform } from "../../platform/platform";
import { useAction, useNow, useResource } from "../../state/useResource";
import "./tray.css";

const MODES: { value: Mode; label: string }[] = (
  ["send", "receive", "both", "paused"] as const
).map((value) => ({ value, label: MODE_LABELS[value] }));

/** The menu-bar popover (macOS) and tray flyout (Windows, KDE) (#42). */
export default function TrayWindow() {
  const status = useResource(getStatus, [EVENTS.status, EVENTS.settings, EVENTS.trayOpened]);
  const settings = useResource(getSettings, [EVENTS.settings]);
  const recent = useResource(
    () => historyList(null, "all", 6),
    [EVENTS.history, EVENTS.expiry, EVENTS.trayOpened],
  );
  const action = useAction();
  const now = useNow(1000);
  const view = status.data;
  const device = view?.thisDevice ?? null;
  const paused = device?.effectiveMode === "paused";

  return (
    <div className="tray">
      <header className="tray__status">
        <StatusDot tone={view?.level ?? "warning"} />
        <div className="tray__status-text">
          <strong>{view?.status ?? "Starting…"}</strong>
          {device?.pausedUntil && (
            <span className="muted"> · paused {pausedFor(device.pausedUntil, now)}</span>
          )}
          {view?.warnings.map((warning) => (
            <div key={warning} className="tray__warning">
              {warning}
            </div>
          ))}
        </div>
        {view?.retry && <Button onClick={() => void action.run(retryKeychain)}>Try again</Button>}
      </header>

      {device && settings.data && (
        <section className="tray__section">
          <div className="tray__label">This device</div>
          <Segmented
            label="Mode of this device"
            options={MODES}
            value={device.mode}
            size="small"
            disabled={action.busy}
            onChange={(mode) =>
              void action.run(() =>
                setDeviceSettings(settings.data!.deviceId, mode, device.ttlSecs),
              )
            }
          />
        </section>
      )}

      <section className="tray__section tray__items">
        <div className="tray__label">
          {platform === "macos" ? "Recent" : "Received from your devices"}
        </div>
        {(recent.data ?? []).length === 0 && (
          <p className="muted tray__empty">
            {platform === "macos" ? "Nothing copied yet." : "Nothing received yet."}
          </p>
        )}
        <ul className="tray__list">
          {(recent.data ?? []).map((item) => (
            <RecentItem
              key={item.id}
              item={item}
              now={now}
              onCopy={() => void action.run(() => historyCopy(item.id, false))}
            />
          ))}
        </ul>
      </section>

      <ErrorText error={action.error} />

      <footer className="tray__actions">
        {platform === "macos" && (
          <button type="button" className="tray__action" onClick={() => void openWindow("panel")}>
            Open Clipboard <span className="muted">⇧⌘V</span>
          </button>
        )}
        <button
          type="button"
          className="tray__action"
          disabled={!device || action.busy}
          onClick={() => void action.run(paused ? resumeSync : pauseForHour)}
        >
          {paused ? "Resume syncing" : "Pause for 1 hour"}
        </button>
        {platform === "windows" && (
          <button
            type="button"
            className="tray__action"
            disabled={action.busy}
            onClick={() => void action.run(clearFromHistory)}
          >
            Clear from Win + V
          </button>
        )}
        <button
          type="button"
          className="tray__action"
          onClick={() => void openWindow("settings", "add-device")}
        >
          Add Device…
        </button>
        <button type="button" className="tray__action" onClick={() => void openWindow("settings")}>
          Settings…
        </button>
        <button type="button" className="tray__action" onClick={() => void quitApp()}>
          Quit Pasta
        </button>
      </footer>
    </div>
  );
}

function RecentItem({
  item,
  now,
  onCopy,
}: {
  item: HistoryItemView;
  now: number;
  onCopy: () => void;
}) {
  const from = item.received
    ? (item.originDevice ?? "another device")
    : (item.sourceApp ?? "this device");
  return (
    <li>
      <button type="button" className="tray__item" onClick={onCopy} title="Copy again">
        <span className="tray__item-text">{item.preview.split("\n")[0]}</span>
        <span className="tray__item-meta muted">
          {from} ·{" "}
          {item.expiresAt !== null ? countdown(item.expiresAt, now) : timeAgo(item.createdAt, now)}
        </span>
      </button>
    </li>
  );
}

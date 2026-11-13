import { useState } from "react";
import {
  clearFromHistory,
  EVENTS,
  expireAllNow,
  getSettings,
  historyClear,
  pauseForHour,
  resumeSync,
  runningApps,
  setAddToHistory,
  setDeviceSettings,
  setHistoryPrefs,
  setPrivacy,
  type ExcludedAppView,
  type Mode,
  type SettingsView,
} from "../../ipc/commands";
import {
  Button,
  Dialog,
  ErrorText,
  InfoBar,
  Row,
  Section,
  Segmented,
  Select,
  Toggle,
} from "../../components/controls";
import {
  MODE_HINTS,
  MODE_LABELS,
  parseTtl,
  pausedFor,
  ttlOptions,
  ttlValue,
} from "../../lib/format";
import { platform } from "../../platform/platform";
import { useAction, useNow, useResource } from "../../state/useResource";

const MODES: { value: Mode; label: string }[] = (
  ["send", "receive", "both", "paused"] as const
).map((value) => ({ value, label: MODE_LABELS[value] }));

const KEEP_ITEMS = [100, 250, 500, 1000, 2500, 5000].map((n) => ({
  value: String(n),
  label: `${n} items`,
}));
const KEEP_DAYS = [
  { value: "1", label: "1 day" },
  { value: "7", label: "1 week" },
  { value: "30", label: "1 month" },
  { value: "90", label: "3 months" },
  { value: "365", label: "1 year" },
  { value: "forever", label: "Until the limit" },
];

/** Settings › Clipboard & expiry (#29, #32, #33, #34, #35, #36). */
export function ClipboardPage() {
  const settings = useResource(getSettings, [EVENTS.settings]);
  if (settings.error && !settings.data) return <InfoBar tone="warning">{settings.error}</InfoBar>;
  if (!settings.data) return <p className="muted">Loading…</p>;
  return (
    <>
      <SyncSection settings={settings.data} />
      {platform === "macos" && <HistorySection settings={settings.data} />}
      <PrivacySection settings={settings.data} />
    </>
  );
}

function SyncSection({ settings }: { settings: SettingsView }) {
  const action = useAction();
  const now = useNow(30_000);
  const [cleared, setCleared] = useState<string | null>(null);
  const device = settings.thisDevice;
  const paused = device.pausedUntil !== null;
  const historyName =
    platform === "windows" ? "Win + V" : platform === "macos" ? "Pasta's history" : "Klipper";

  return (
    <Section title="This device">
      <Row label="Mode" hint={MODE_HINTS[device.mode]}>
        <Segmented
          label="Mode of this device"
          options={MODES}
          value={device.mode}
          disabled={action.busy}
          onChange={(mode) =>
            void action.run(() => setDeviceSettings(settings.deviceId, mode, device.ttlSecs))
          }
        />
      </Row>
      <Row
        label="Pause syncing"
        hint={
          paused && device.pausedUntil
            ? `Paused ${pausedFor(device.pausedUntil, now)}.`
            : "Stops sending and receiving for an hour. Nothing is caught up afterwards: copies made meanwhile stay on this device."
        }
      >
        {paused ? (
          <Button disabled={action.busy} onClick={() => void action.run(resumeSync)}>
            Resume
          </Button>
        ) : (
          <Button disabled={action.busy} onClick={() => void action.run(pauseForHour)}>
            Pause for 1 hour
          </Button>
        )}
      </Row>
      {platform !== "macos" && (
        <Row
          label={`Add received items to ${historyName}`}
          hint={
            platform === "linux"
              ? "Off: items only become the current clipboard, so expiry removes them completely."
              : "Off: items only become the current clipboard."
          }
        >
          <Toggle
            label={`Add received items to ${historyName}`}
            checked={settings.addToHistory}
            onChange={(on) => void action.run(() => setAddToHistory(on))}
          />
        </Row>
      )}
      <Row
        label="Expire received items after"
        hint={
          platform === "linux" && settings.addToHistory
            ? "Items from your other devices leave the clipboard after this. Klipper can't remove a single entry, so they stay in its history until you delete them there."
            : `Items from your other devices are removed from ${historyName} and the clipboard after this.`
        }
      >
        <Select
          label="Expire received items after"
          value={ttlValue(device.ttlSecs)}
          options={ttlOptions(settings.ttlChoices)}
          disabled={action.busy}
          onChange={(value) =>
            void action.run(() =>
              setDeviceSettings(settings.deviceId, device.mode, parseTtl(value)),
            )
          }
        />
      </Row>
      <Row
        label="Expire all now"
        hint={
          platform === "linux"
            ? "Clears every received item from the clipboard at once."
            : "Removes every received item at once."
        }
      >
        <Button
          disabled={action.busy}
          onClick={() =>
            void action.run(async () => {
              const count = await expireAllNow();
              setCleared(`${count} ${count === 1 ? "item" : "items"} expired.`);
            })
          }
        >
          Expire all now
        </Button>
        {platform === "windows" && (
          <Button
            disabled={action.busy}
            onClick={() =>
              void action.run(async () => {
                const count = await clearFromHistory();
                setCleared(`${count} ${count === 1 ? "item" : "items"} removed from Win + V.`);
              })
            }
          >
            Clear from Win + V
          </Button>
        )}
      </Row>
      {cleared && <p className="muted pad">{cleared}</p>}
      <ErrorText error={action.error} />
      {platform === "windows" && (
        <div className="pad">
          <InfoBar tone="info" title="After a restart">
            Windows clears unpinned Win + V items when it restarts, so items that were waiting to
            expire are simply gone. Pinned items stay until you unpin them.
          </InfoBar>
        </div>
      )}
    </Section>
  );
}

function HistorySection({ settings }: { settings: SettingsView }) {
  const action = useAction();
  const [confirm, setConfirm] = useState(false);
  const keepDays = settings.historyKeepDays === null ? "forever" : String(settings.historyKeepDays);
  const save = (maxItems: number, days: number | null) =>
    void action.run(() => setHistoryPrefs(maxItems, days));
  return (
    <Section
      title="Clipboard history"
      description="Kept on this Mac only, encrypted with a key in your Keychain."
    >
      <Row label="Keep up to">
        <Select
          label="Keep up to"
          value={String(settings.historyMaxItems)}
          options={
            KEEP_ITEMS.some((o) => o.value === String(settings.historyMaxItems))
              ? KEEP_ITEMS
              : [
                  ...KEEP_ITEMS,
                  {
                    value: String(settings.historyMaxItems),
                    label: `${settings.historyMaxItems} items`,
                  },
                ]
          }
          onChange={(value) => save(Number(value), settings.historyKeepDays)}
        />
      </Row>
      <Row
        label="Keep items copied here for"
        hint="Items from other devices follow the expiry above."
      >
        <Select
          label="Keep items copied here for"
          value={keepDays}
          options={KEEP_DAYS}
          onChange={(value) =>
            save(settings.historyMaxItems, value === "forever" ? null : Number(value))
          }
        />
      </Row>
      <Row label="Clear history">
        <Button variant="danger" onClick={() => setConfirm(true)}>
          Clear history…
        </Button>
      </Row>
      <ErrorText error={action.error} />
      <Dialog
        open={confirm}
        title="Clear the clipboard history?"
        onClose={() => setConfirm(false)}
        footer={
          <>
            <Button onClick={() => setConfirm(false)}>Cancel</Button>
            <Button
              variant="primary"
              onClick={() =>
                void action.run(async () => {
                  await historyClear();
                  setConfirm(false);
                })
              }
            >
              Clear
            </Button>
          </>
        }
      >
        <p>Every item in the panel is deleted from this Mac. Your other devices keep theirs.</p>
      </Dialog>
    </Section>
  );
}

function PrivacySection({ settings }: { settings: SettingsView }) {
  const action = useAction();
  const save = (skip: boolean, apps: ExcludedAppView[]) =>
    void action.run(() => setPrivacy(skip, apps));
  const idLabel = platform === "macos" ? "Bundle id" : "Program (.exe)";

  return (
    <Section
      title="Privacy"
      description="Items Pasta skips are never sent to your other devices and never stored."
    >
      <Row
        label="Skip passwords and other sensitive items"
        hint={
          platform === "linux"
            ? "Items password managers such as KeePassXC mark as secret."
            : `Items password managers mark as concealed, and copies in ${
                settings.builtinExcluded.length > 0
                  ? `${settings.builtinExcluded.length} known password managers`
                  : "known password managers"
              }.`
        }
      >
        <Toggle
          label="Skip passwords and other sensitive items"
          checked={settings.skipSensitive}
          onChange={(on) => save(on, settings.excludedApps)}
        />
      </Row>
      {platform === "linux" ? (
        <Row
          label="Excluded apps"
          hint="Not available on Plasma: Wayland doesn't tell Pasta which app copied. Pause syncing instead while you work with something private."
        />
      ) : (
        <ExcludedApps settings={settings} save={save} idLabel={idLabel} />
      )}
      <ErrorText error={action.error} />
    </Section>
  );
}

function ExcludedApps({
  settings,
  save,
  idLabel,
}: {
  settings: SettingsView;
  save: (skip: boolean, apps: ExcludedAppView[]) => void;
  idLabel: string;
}) {
  const [picking, setPicking] = useState(false);
  return (
    <>
      {settings.excludedApps.map((app) => (
        <Row key={app.id} label={app.name} hint={<span className="mono">{app.id}</span>}>
          <Button
            variant="plain"
            onClick={() =>
              save(
                settings.skipSensitive,
                settings.excludedApps.filter((other) => other.id !== app.id),
              )
            }
          >
            Remove
          </Button>
        </Row>
      ))}
      <Row
        label="Excluded apps"
        hint={
          settings.excludedApps.length === 0
            ? "Copies in these apps stay on this device."
            : undefined
        }
      >
        <Button onClick={() => setPicking(true)}>Add app…</Button>
      </Row>
      {picking && (
        <AppPicker
          idLabel={idLabel}
          onCancel={() => setPicking(false)}
          onPick={(app) => {
            setPicking(false);
            save(settings.skipSensitive, [...settings.excludedApps, app]);
          }}
        />
      )}
    </>
  );
}

function AppPicker({
  idLabel,
  onPick,
  onCancel,
}: {
  idLabel: string;
  onPick: (app: ExcludedAppView) => void;
  onCancel: () => void;
}) {
  const apps = useResource(runningApps, []);
  const [id, setId] = useState("");
  const [name, setName] = useState("");
  return (
    <Dialog
      open
      title="Exclude an app"
      onClose={onCancel}
      footer={
        <>
          <Button onClick={onCancel}>Cancel</Button>
          <Button
            variant="primary"
            disabled={id.trim() === ""}
            onClick={() => onPick({ id: id.trim(), name: name.trim() || id.trim() })}
          >
            Exclude
          </Button>
        </>
      }
    >
      <p className="muted">Pick an open app, or type its {idLabel.toLowerCase()}.</p>
      <ul className="app-list" aria-label="Open apps">
        {(apps.data ?? []).map((app) => (
          <li key={app.id}>
            <button type="button" onClick={() => onPick(app)}>
              <span>{app.name || app.id}</span>
              <span className="muted mono">{app.id}</span>
            </button>
          </li>
        ))}
        {apps.data?.length === 0 && <li className="muted">No open apps found.</li>}
      </ul>
      <label className="field">
        <span>{idLabel}</span>
        <input value={id} onChange={(event) => setId(event.target.value)} spellCheck={false} />
      </label>
      <label className="field">
        <span>Name</span>
        <input value={name} onChange={(event) => setName(event.target.value)} />
      </label>
    </Dialog>
  );
}

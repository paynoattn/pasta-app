import { useState } from "react";
import {
  checkUpdates,
  EVENTS,
  setOpenAtLogin,
  setUpdatePrefs,
  startupStatus,
} from "../../ipc/commands";
import {
  Button,
  ErrorText,
  InfoBar,
  Row,
  Section,
  Select,
  Toggle,
} from "../../components/controls";
import { useAction, useResource } from "../../state/useResource";

const INTERVALS = [
  { value: "6", label: "Every 6 hours" },
  { value: "24", label: "Daily" },
  { value: "168", label: "Weekly" },
];

/** Settings › Startup & updates (#47). */
export function StartupPage() {
  const status = useResource(startupStatus, [EVENTS.update, EVENTS.settings]);
  const action = useAction();
  const [checked, setChecked] = useState<string | null>(null);
  if (status.error && !status.data) return <InfoBar tone="warning">{status.error}</InfoBar>;
  if (!status.data) return <p className="muted">Loading…</p>;
  const view = status.data;
  const interval = INTERVALS.some((option) => option.value === String(view.checkHours))
    ? String(view.checkHours)
    : "24";

  return (
    <>
      <Section title="Startup">
        <Row label="Open at login" hint="Pasta starts in the background when you sign in.">
          <Toggle
            label="Open at login"
            checked={view.openAtLogin}
            disabled={action.busy}
            onChange={(on) =>
              void action.run(async () => {
                await setOpenAtLogin(on);
                status.reload();
              })
            }
          />
        </Row>
      </Section>
      <Section title="Updates">
        {!view.updaterConfigured && (
          <div className="pad">
            <InfoBar tone="info" title="Updates aren't set up for this build yet.">
              Signed updates arrive with the release builds. Until then, install new versions by
              hand.
            </InfoBar>
          </div>
        )}
        <Row
          label="Update automatically"
          hint="Downloads and installs signed updates; they apply next time Pasta starts."
        >
          <Toggle
            label="Update automatically"
            checked={view.autoUpdate}
            disabled={action.busy || !view.updaterConfigured}
            onChange={(on) => void action.run(() => setUpdatePrefs(on, view.checkHours))}
          />
        </Row>
        <Row label="Check for updates">
          <Select
            label="How often to check"
            value={interval}
            options={INTERVALS}
            disabled={!view.updaterConfigured}
            onChange={(value) =>
              void action.run(() => setUpdatePrefs(view.autoUpdate, Number(value)))
            }
          />
          <Button
            disabled={action.busy || !view.updaterConfigured}
            onClick={() =>
              void action.run(async () => {
                const version = await checkUpdates(false);
                setChecked(version ? `Pasta ${version} is available.` : "Pasta is up to date.");
              })
            }
          >
            Check now
          </Button>
          {view.availableVersion && view.updaterConfigured && (
            <Button
              variant="primary"
              disabled={action.busy}
              onClick={() => void action.run(() => checkUpdates(true))}
            >
              Install {view.availableVersion}
            </Button>
          )}
        </Row>
        {(checked ?? view.updateStatus) && view.updaterConfigured && (
          <p className="muted pad">{checked ?? view.updateStatus}</p>
        )}
        <ErrorText error={action.error} />
      </Section>
    </>
  );
}

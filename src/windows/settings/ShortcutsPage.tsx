import { useEffect, useState } from "react";
import {
  setPanelShortcut,
  shortcutsStatus,
  suspendShortcut,
  type ShortcutsView,
} from "../../ipc/commands";
import { Button, ErrorText, InfoBar, Row, Section } from "../../components/controls";
import { displayAccelerator, fromKeyboardEvent } from "../../lib/accelerator";
import { platform } from "../../platform/platform";
import { useAction, useResource } from "../../state/useResource";

/** Settings › Shortcuts (#48). */
export function ShortcutsPage() {
  const status = useResource(shortcutsStatus, []);
  const [view, setView] = useState<ShortcutsView | null>(null);
  const current = view ?? status.data;
  if (status.error && !current) return <InfoBar tone="warning">{status.error}</InfoBar>;
  if (!current) return <p className="muted">Loading…</p>;

  if (!current.supported) {
    return (
      <Section title="History panel">
        <Row
          label="Open the history panel"
          hint={
            platform === "windows"
              ? "Use Win + V for now. Pasta's own history panel for Windows comes later."
              : "Use Klipper's shortcut for now. Pasta's own history panel for KDE comes later."
          }
        >
          <span className="shortcut shortcut--disabled">Not set</span>
        </Row>
      </Section>
    );
  }
  return <PanelShortcut view={current} onChange={setView} />;
}

function PanelShortcut({
  view,
  onChange,
}: {
  view: ShortcutsView;
  onChange: (view: ShortcutsView) => void;
}) {
  const [recording, setRecording] = useState(false);
  const action = useAction();
  const { run } = action;

  useEffect(() => {
    if (!recording) return;
    // The current shortcut would open the panel instead of being recorded.
    void suspendShortcut(true);
    const onKey = (event: KeyboardEvent) => {
      event.preventDefault();
      event.stopPropagation();
      if (event.key === "Escape" && !event.metaKey && !event.ctrlKey && !event.altKey) {
        setRecording(false);
        return;
      }
      const accelerator = fromKeyboardEvent(event);
      if (!accelerator) return;
      setRecording(false);
      void run(async () => onChange(await setPanelShortcut(accelerator)));
    };
    window.addEventListener("keydown", onKey, true);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      void suspendShortcut(false);
    };
  }, [recording, run, onChange]);

  return (
    <Section title="Clipboard panel">
      <Row label="Open the clipboard panel" hint="Works in every app, including full-screen ones.">
        <span className={`shortcut${recording ? " shortcut--recording" : ""}`} aria-live="polite">
          {recording
            ? "Press a shortcut…"
            : view.panel
              ? displayAccelerator(view.panel)
              : "Not set"}
        </span>
        <Button onClick={() => setRecording(!recording)} disabled={action.busy}>
          {recording ? "Cancel" : "Record"}
        </Button>
        {view.panel !== view.defaultPanel && (
          <Button
            variant="plain"
            disabled={action.busy}
            onClick={() => void action.run(async () => onChange(await setPanelShortcut(null)))}
          >
            Reset
          </Button>
        )}
      </Row>
      <div className="pad">
        {view.error && <InfoBar tone="error">{view.error}</InfoBar>}
        {view.warning && <InfoBar tone="warning">{view.warning}</InfoBar>}
        <ErrorText error={action.error} />
      </div>
    </Section>
  );
}

import { useState } from "react";
import { shortcutsStatus, type ShortcutsView } from "../../ipc/commands";
import { Button, ErrorText, InfoBar, Row, Section } from "../../components/controls";
import { ShortcutChip } from "../../components/ShortcutChip";
import { useShortcutRecorder } from "../../state/useShortcutRecorder";
import { platform } from "../../platform/platform";
import { useResource } from "../../state/useResource";

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
  const recorder = useShortcutRecorder(onChange);
  return (
    <Section title="Clipboard panel">
      <Row
        label="Open the clipboard panel"
        hint="Works in every app, including full-screen ones. Click the shortcut to change it."
      >
        <ShortcutChip view={view} recorder={recorder} />
        {view.panel !== view.defaultPanel && !recorder.recording && (
          <Button variant="plain" disabled={recorder.busy} onClick={recorder.reset}>
            Reset
          </Button>
        )}
      </Row>
      <div className="pad">
        {view.error && <InfoBar tone="error">{view.error}</InfoBar>}
        {view.warning && <InfoBar tone="warning">{view.warning}</InfoBar>}
        <ErrorText error={recorder.error} />
      </div>
    </Section>
  );
}

import type { ShortcutsView } from "../ipc/commands";
import { displayAccelerator } from "../lib/accelerator";
import type { ShortcutRecorder } from "../state/useShortcutRecorder";

/** The shortcut as a key cap; clicking it records a new one. */
export function ShortcutChip({
  view,
  recorder,
}: {
  view: ShortcutsView;
  recorder: ShortcutRecorder;
}) {
  const shown = view.panel ? displayAccelerator(view.panel) : "Not set";
  return (
    <button
      type="button"
      className={`shortcut shortcut--button${recorder.recording ? " shortcut--recording" : ""}`}
      aria-label={
        recorder.recording
          ? "Press the new shortcut, or Esc to cancel"
          : `Change the panel shortcut, now ${shown}`
      }
      title={recorder.recording ? "Esc to cancel" : "Click, then press the new shortcut"}
      aria-live="polite"
      disabled={recorder.busy}
      onClick={recorder.toggle}
    >
      {recorder.recording ? "Press a shortcut…" : shown}
    </button>
  );
}

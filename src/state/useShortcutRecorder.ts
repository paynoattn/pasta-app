import { useEffect, useState } from "react";
import { setPanelShortcut, suspendShortcut, type ShortcutsView } from "../ipc/commands";
import { fromKeyboardEvent } from "../lib/accelerator";
import { useAction } from "./useResource";

export interface ShortcutRecorder {
  recording: boolean;
  /** Starts recording, or cancels it. */
  toggle: () => void;
  /** Back to the default shortcut. */
  reset: () => void;
  busy: boolean;
  error: string | null;
}

/**
 * Records the panel shortcut from the next key press (#48): the current one is
 * suspended meanwhile, so pressing it records it instead of opening the panel.
 * Esc cancels.
 */
export function useShortcutRecorder(onChange: (view: ShortcutsView) => void): ShortcutRecorder {
  const [recording, setRecording] = useState(false);
  const action = useAction();
  const { run } = action;

  useEffect(() => {
    if (!recording) return;
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

  return {
    recording,
    toggle: () => setRecording((on) => !on),
    reset: () => void run(async () => onChange(await setPanelShortcut(null))),
    busy: action.busy,
    error: action.error,
  };
}

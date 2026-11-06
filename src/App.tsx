import { lazy, Suspense } from "react";

// One bundle for every window; each loads only its own tree (#28).
const SettingsWindow = lazy(() => import("./windows/settings/SettingsWindow"));
const SetupWindow = lazy(() => import("./windows/setup/SetupWindow"));
const TrayWindow = lazy(() => import("./windows/tray/TrayWindow"));
const PanelWindow = lazy(() => import("./windows/panel/PanelWindow"));

/** The component tree for the window with Tauri label `label`. */
export function App({ label }: { label: string }) {
  return (
    <Suspense fallback={null}>
      {label === "setup" ? (
        <SetupWindow />
      ) : label === "tray" ? (
        <TrayWindow />
      ) : label === "panel" ? (
        <PanelWindow />
      ) : (
        <SettingsWindow />
      )}
    </Suspense>
  );
}

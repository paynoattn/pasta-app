import { useEffect, useState } from "react";
import { getAppInfo, type AppInfo } from "../ipc/commands";
import { DevChainPanel } from "./DevChainPanel";
import "./SettingsWindow.css";

/** Placeholder until the settings shell lands (#29). */
export function SettingsWindow() {
  const [info, setInfo] = useState<AppInfo | null>(null);

  useEffect(() => {
    getAppInfo()
      .then(setInfo)
      .catch(() => setInfo(null));
  }, []);

  return (
    <main className="settings">
      <h1>Pasta</h1>
      <p>Clipboard sync across your devices over Tailscale.</p>
      {info && (
        <p className="settings__meta">
          Version {info.version} · protocol v{info.protocolVersion}
        </p>
      )}
      <DevChainPanel />
    </main>
  );
}

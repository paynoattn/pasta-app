import React from "react";
import ReactDOM from "react-dom/client";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { App } from "./App";
import { applyPlatform } from "./platform/platform";
import "./platform/theme.css";

function windowLabel(): string {
  try {
    return getCurrentWebviewWindow().label;
  } catch {
    return "main"; // a plain browser (vite dev without Tauri)
  }
}

const label = windowLabel();
applyPlatform();
if (label === "tray" || label === "panel") {
  document.documentElement.classList.add("transparent");
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App label={label} />
  </React.StrictMode>,
);

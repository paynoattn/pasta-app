import { useState } from "react";
import {
  copyDiagnostics,
  diagnosticsReport,
  getAppInfo,
  openLogFolder,
  openWindow,
} from "../../ipc/commands";
import { Button, ErrorText, Row, Section } from "../../components/controls";
import { useAction, useResource } from "../../state/useResource";

/** Settings › About (#29, #18). */
export function AboutPage() {
  const info = useResource(getAppInfo, []);
  const report = useResource(diagnosticsReport, []);
  const action = useAction();
  const [copied, setCopied] = useState(false);
  return (
    <>
      <Section>
        <Row label="Pasta" hint="Clipboard sync across your devices over Tailscale.">
          {info.data && (
            <span className="muted">
              Version {info.data.version} · protocol v{info.data.protocolVersion}
            </span>
          )}
        </Row>
        <Row
          label="Setup"
          hint="Go through the setup steps again, e.g. after reinstalling Tailscale."
        >
          <Button onClick={() => void openWindow("setup")}>Run setup again…</Button>
        </Row>
      </Section>
      <Section
        title="Diagnostics"
        description="For bug reports: versions, states and counts. No device names, addresses or clipboard content."
      >
        <pre className="diagnostics selectable">{report.data ?? report.error ?? "…"}</pre>
        <div className="row">
          <div className="row__text" />
          <div className="row__control">
            <Button onClick={() => void action.run(openLogFolder)}>Open log folder</Button>
            <Button
              variant="primary"
              onClick={() =>
                void action.run(async () => {
                  await copyDiagnostics();
                  setCopied(true);
                })
              }
            >
              {copied ? "Copied" : "Copy diagnostics"}
            </Button>
          </div>
        </div>
        <ErrorText error={action.error} />
      </Section>
    </>
  );
}

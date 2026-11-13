import { useState } from "react";
import {
  EVENTS,
  finishSetup,
  openSettingsLink,
  requestAccessibility,
  setIcloudBridge,
  setOpenAtLogin,
  setUpdatePrefs,
  setupStatus,
  shortcutsStatus,
  startupStatus,
  type SetupView,
  type ShortcutsView,
} from "../../ipc/commands";
import { ChainChoice } from "../../components/chain";
import { Button, ErrorText, InfoBar, Row, Section, Toggle } from "../../components/controls";
import { ShortcutChip } from "../../components/ShortcutChip";
import { useShortcutRecorder } from "../../state/useShortcutRecorder";
import { displayAccelerator } from "../../lib/accelerator";
import { platform } from "../../platform/platform";
import { useAction, useNow, useResource } from "../../state/useResource";
import "./setup.css";

const STEPS = [
  "Welcome",
  "Connect Tailscale",
  "Create or join a chain",
  "Permissions & startup",
  "All set",
] as const;

/** The setup flow (#43–#45): re-runnable from Settings › About. */
export default function SetupWindow() {
  const [step, setStep] = useState(0);
  // Polled while open, so granting a permission or starting Tailscale shows up live.
  const tick = useNow(2000);
  const status = useResource(
    setupStatus,
    [EVENTS.status, EVENTS.chain, EVENTS.settings],
    String(tick),
  );
  const view = status.data;

  return (
    <div className="setup">
      <ol className="setup__steps" aria-label="Setup steps">
        {STEPS.map((name, index) => (
          <li
            key={name}
            className={`setup__step${index === step ? " setup__step--on" : ""}${index < step ? " setup__step--done" : ""}`}
            aria-current={index === step ? "step" : undefined}
          >
            <span className="setup__step-number">{index < step ? "✓" : index + 1}</span>
            {name}
          </li>
        ))}
      </ol>
      <main className="setup__page">
        {!view ? (
          status.error ? (
            <InfoBar tone="warning">{status.error}</InfoBar>
          ) : (
            <p className="muted">Loading…</p>
          )
        ) : (
          <>
            {step === 0 && <Welcome />}
            {step === 1 && <TailscaleStep view={view} />}
            {step === 2 && <ChainStep view={view} onDone={() => setStep(3)} />}
            {step === 3 && <PermissionsStep view={view} />}
            {step === 4 && <DoneStep view={view} />}
            <div className="setup__nav">
              {step > 0 && <Button onClick={() => setStep(step - 1)}>Back</Button>}
              <span className="setup__spacer" />
              {step < STEPS.length - 1 ? (
                <Button
                  variant="primary"
                  onClick={() => setStep(step + 1)}
                  disabled={step === 2 && view.chain === "none"}
                >
                  {step === 1 && view.tailscale.state !== "running"
                    ? "Continue anyway"
                    : "Continue"}
                </Button>
              ) : (
                <FinishButton />
              )}
            </div>
          </>
        )}
      </main>
    </div>
  );
}

function FinishButton() {
  const action = useAction();
  return (
    <>
      <ErrorText error={action.error} />
      <Button variant="primary" disabled={action.busy} onClick={() => void action.run(finishSetup)}>
        Finish
      </Button>
    </>
  );
}

function Welcome() {
  return (
    <div className="setup__intro">
      <h1>Welcome to Pasta</h1>
      <p>
        Pasta sends what you copy to your other computers, so it shows up in their clipboard
        history:
        {platform === "windows"
          ? " Win + V here,"
          : platform === "macos"
            ? " Pasta's panel (⇧⌘V) here,"
            : " Klipper here,"}{" "}
        and the same on your Macs, PCs and KDE desktops.
      </p>
      <ul className="checklist">
        <li>It travels only over your Tailscale network, end-to-end encrypted.</li>
        <li>Each device chooses whether it sends, receives, or both, and how long items stay.</li>
        <li>Passwords from password managers are skipped.</li>
      </ul>
    </div>
  );
}

function TailscaleStep({ view }: { view: SetupView }) {
  const tailscale = view.tailscale;
  // macOS without Tailscale.app (Homebrew's `tailscale` formula): Terminal, not the app.
  const cliOnly = platform === "macos" && tailscale.cliOnly;
  return (
    <>
      <h1>Connect Tailscale</h1>
      <p>
        Pasta finds your other devices through Tailscale and only talks to them over your tailnet.
      </p>
      {tailscale.state === "running" && (
        <InfoBar tone="success" title="Tailscale is connected.">
          This device is {tailscale.address ?? "on your tailnet"}.
        </InfoBar>
      )}
      {tailscale.state === "starting" && <InfoBar tone="info">Checking Tailscale…</InfoBar>}
      {tailscale.state === "missing" &&
        (cliOnly ? (
          <InfoBar tone="error" title="Tailscale's service isn't running.">
            In Terminal, start it with <code>sudo brew services start tailscale</code> (if you
            installed it with Homebrew), then run <code>tailscale up</code> and sign in.
            {tailscale.detail && <span className="muted"> ({tailscale.detail})</span>}
          </InfoBar>
        ) : (
          <InfoBar
            tone="error"
            title="Tailscale isn't installed or isn't running."
            action={
              <Button onClick={() => void openSettingsLink("tailscale_download")}>
                Get Tailscale
              </Button>
            }
          >
            Install it, sign in, then come back here.
            {tailscale.detail && <span className="muted"> ({tailscale.detail})</span>}
          </InfoBar>
        ))}
      {tailscale.state === "stopped" && (
        <InfoBar
          tone="warning"
          title={
            tailscale.detail?.includes("Login")
              ? "Sign in to Tailscale."
              : "Tailscale is disconnected."
          }
          action={
            platform === "macos" && !cliOnly ? (
              <Button onClick={() => void openSettingsLink("tailscale_app")}>Open Tailscale</Button>
            ) : undefined
          }
        >
          {platform === "windows" ? (
            "Click the Tailscale icon in the taskbar and connect."
          ) : cliOnly ? (
            <>
              In Terminal, run <code>tailscale up</code> and sign in.
            </>
          ) : platform === "macos" ? (
            "Open Tailscale from the menu bar and connect."
          ) : (
            <>
              In a terminal, run <code>sudo tailscale up</code> and sign in.
            </>
          )}
          {tailscale.detail && <span className="muted"> ({tailscale.detail})</span>}
        </InfoBar>
      )}
      {platform === "linux" && tailscale.state !== "running" && deniedAccess(tailscale.detail) && (
        <InfoBar tone="warning" title="Tailscale won't answer Pasta.">
          Run <code>sudo tailscale set --operator=$USER</code> once, then come back here.
        </InfoBar>
      )}
      <p className="muted">Use the same Tailscale account on every device you want to sync.</p>
    </>
  );
}

/** Linux: tailscaled refused the CLI, which `--operator` fixes. */
function deniedAccess(detail: string | null): boolean {
  return /denied|permission|operator/i.test(detail ?? "");
}

function ChainStep({ view, onDone }: { view: SetupView; onDone: () => void }) {
  // Kept on screen after creating, until the new words have been seen.
  const [creating, setCreating] = useState(false);
  if (view.chain === "none" || creating) {
    return (
      <>
        <h1>Create or join a chain</h1>
        <p>Your devices sync only within one sync chain, protected by 24 words.</p>
        {/* The setup flow's own Continue moves on, so ChainChoice doesn't add a second. */}
        <ChainChoice
          onCreateStart={() => setCreating(true)}
          onDone={onDone}
          continueButton={false}
        />
      </>
    );
  }
  return (
    <>
      <h1>Create or join a chain</h1>
      <p>Your devices sync only within one sync chain, protected by 24 words.</p>
      {view.chain === "waiting" && (
        <InfoBar tone="info" title="Waiting for approval.">
          On one of your other devices, open Pasta › Devices and approve this one. You can carry on
          meanwhile.
        </InfoBar>
      )}
      {view.chain === "member" && (
        <InfoBar tone="success" title="This device is in your sync chain." />
      )}
    </>
  );
}

function PermissionsStep({ view }: { view: SetupView }) {
  const action = useAction();
  const startup = useResource(startupStatus, [EVENTS.update]);
  return (
    <>
      <h1>Permissions & startup</h1>
      {view.os === "macos" && (
        <Section title="macOS">
          <Row
            label="Paste into other apps"
            hint={
              view.accessibility
                ? "Granted. Picking an item in the panel pastes it where you were typing."
                : "Allow Pasta under Privacy & Security › Accessibility. Without it, picking an item only copies it."
            }
          >
            {view.accessibility ? (
              <span className="granted">✓ Allowed</span>
            ) : (
              <>
                <Button onClick={() => void action.run(requestAccessibility)}>Ask macOS</Button>
                <Button onClick={() => void action.run(() => openSettingsLink("accessibility"))}>
                  Open System Settings
                </Button>
              </>
            )}
          </Row>
          <PanelShortcutRow />
          <Row
            label="Bridge iCloud Universal Clipboard"
            hint="Copies from your iPhone or iPad go to your other devices too, and theirs reach your iPhone."
          >
            <Toggle
              label="Bridge iCloud Universal Clipboard"
              checked={view.icloudBridge}
              onChange={(on) => void action.run(() => setIcloudBridge(on))}
            />
          </Row>
        </Section>
      )}
      {view.os === "windows" && (
        <Section title="Windows">
          <Row
            label="Clipboard history"
            hint={
              view.clipboardHistory === false
                ? "Off: items from your other devices won't show in Win + V."
                : "On: items from your other devices show in Win + V."
            }
          >
            {view.clipboardHistory === false ? (
              <Button onClick={() => void action.run(() => openSettingsLink("clipboard"))}>
                Turn it on…
              </Button>
            ) : (
              <span className="granted">✓ On</span>
            )}
          </Row>
          {view.cloudSync && (
            <div className="pad">
              <InfoBar
                tone="warning"
                title="Windows is syncing your clipboard too."
                action={
                  <Button onClick={() => void action.run(() => openSettingsLink("clipboard"))}>
                    Open settings
                  </Button>
                }
              >
                “Sync across your devices” is on, so items may show up twice. Turn it off to let
                Pasta handle syncing.
              </InfoBar>
            </div>
          )}
          <Row
            label="Firewall"
            hint="If Windows asks whether Pasta may use the network, allow it on private networks. Pasta only listens on the Tailscale interface."
          >
            <Button onClick={() => void action.run(() => openSettingsLink("firewall"))}>
              Firewall settings…
            </Button>
          </Row>
          <Row
            label="Keep Pasta in the taskbar"
            hint="Drag Pasta's icon from the hidden icons (^) onto the taskbar, or turn it on under Settings › Personalization › Taskbar."
          />
        </Section>
      )}
      {view.os === "linux" && (
        <Section title="KDE Plasma">
          {view.clipboardUnsupported && (
            <div className="pad">
              <InfoBar tone="error" title="Pasta can't sync on this desktop.">
                {view.clipboardUnsupported}.
              </InfoBar>
            </div>
          )}
          {view.clipboardHistory !== null && (
            <Row
              label="Klipper"
              hint={
                view.clipboardHistory === false
                  ? "Not running: items from your other devices only become the current clipboard. Add the Clipboard widget to the system tray to get its history back."
                  : "Running: items from your other devices show in Klipper (Meta + V)."
              }
            >
              {view.clipboardHistory === false ? (
                <span className="muted">Not running</span>
              ) : (
                <span className="granted">✓ Running</span>
              )}
            </Row>
          )}
          <Row
            label="Keep Pasta in the system tray"
            hint="If its icon is under the arrow (^), right-click the arrow › Configure System Tray… › Entries, and set Pasta to Always shown."
          />
        </Section>
      )}
      <Section title="Startup">
        <Row label="Open at login" hint="Pasta starts quietly in the background when you sign in.">
          <Toggle
            label="Open at login"
            checked={view.openAtLogin}
            onChange={(on) => void action.run(() => setOpenAtLogin(on))}
          />
        </Row>
        <Row
          label="Update automatically"
          hint={
            startup.data && !startup.data.updaterConfigured
              ? "Your choice is kept; updates start with the release builds."
              : "Installs signed updates; they apply next time Pasta starts."
          }
        >
          <Toggle
            label="Update automatically"
            checked={view.autoUpdate}
            onChange={(on) =>
              void action.run(() => setUpdatePrefs(on, startup.data?.checkHours ?? 24))
            }
          />
        </Row>
      </Section>
      <ErrorText error={action.error} />
    </>
  );
}

/** macOS: the panel shortcut, changed by clicking it (#44, #48). */
function PanelShortcutRow() {
  const status = useResource(shortcutsStatus, []);
  const [changed, setChanged] = useState<ShortcutsView | null>(null);
  // A state setter, so the recorder's callback stays the same across the 2 s polls.
  const recorder = useShortcutRecorder(setChanged);
  const view = changed ?? status.data;
  return (
    <>
      <Row
        label="Clipboard panel shortcut"
        hint="Many apps use ⇧⌘V for Paste and Match Style; Pasta takes it over while it runs. Click the shortcut to change it."
      >
        {view ? (
          <ShortcutChip view={view} recorder={recorder} />
        ) : (
          <span className="muted">{status.error ?? "…"}</span>
        )}
      </Row>
      {view && (view.error || view.warning || recorder.error) && (
        <div className="pad">
          {view.error && <InfoBar tone="error">{view.error}</InfoBar>}
          {view.warning && <InfoBar tone="warning">{view.warning}</InfoBar>}
          <ErrorText error={recorder.error} />
        </div>
      )}
    </>
  );
}

function DoneStep({ view }: { view: SetupView }) {
  return (
    <div className="setup__intro">
      <h1>All set</h1>
      <ul className="checklist">
        {view.tailscale.state === "running" && <li>Tailscale is connected.</li>}
        {view.chain === "member" && <li>This device is in your sync chain.</li>}
        {view.chain === "waiting" && (
          <li>This device waits for approval on one of your other devices.</li>
        )}
        {view.os === "macos" && (
          <li>
            Press {displayAccelerator(view.panelShortcut ?? "Shift+Command+V")} to open your
            clipboard history.
          </li>
        )}
        {view.os === "windows" && <li>Press Win + V to see items from your other devices.</li>}
        {view.os === "linux" && view.clipboardHistory === true && (
          <li>Press Meta + V to see items from your other devices in Klipper.</li>
        )}
      </ul>
      <p className="muted">
        Pasta lives in the {platform === "macos" ? "menu bar" : "system tray"}. Settings are there
        too.
      </p>
    </div>
  );
}

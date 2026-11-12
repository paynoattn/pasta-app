import { useState } from "react";
import {
  approveDevice,
  declineDevice,
  getChain,
  removeDevice,
  type ChainMember,
  type PendingDevice,
} from "../../ipc/chain";
import {
  EVENTS,
  getSettings,
  listPeers,
  openWindow,
  setDeviceName,
  setDeviceSettings,
  setIcloudBridge,
  setupStatus,
  type DeviceSettingsView,
  type Mode,
  type Peer,
} from "../../ipc/commands";
import { AddDeviceDialog } from "../../components/AddDeviceDialog";
import {
  Badge,
  Button,
  Dialog,
  ErrorText,
  InfoBar,
  Row,
  Section,
  Segmented,
  Select,
  StatusDot,
  Toggle,
} from "../../components/controls";
import { MODE_LABELS, osLabel, parseTtl, ttlOptions, ttlValue } from "../../lib/format";
import { platform } from "../../platform/platform";
import { useAction, useResource } from "../../state/useResource";

const MODES: { value: Mode; label: string }[] = (
  ["send", "receive", "both", "paused"] as const
).map((value) => ({ value, label: MODE_LABELS[value] }));

/** Settings › Devices (#30): the chain, pending approvals and every device's mode and expiry. */
export function DevicesPage({
  addDevice,
  onAddDeviceChange,
}: {
  addDevice: boolean;
  onAddDeviceChange: (open: boolean) => void;
}) {
  const chain = useResource(getChain, [EVENTS.chain]);
  const peers = useResource(listPeers, [EVENTS.peers]);
  const settings = useResource(getSettings, [EVENTS.settings]);
  const setup = useResource(setupStatus, [EVENTS.status]);
  const bridge = useAction();

  if (chain.error && !chain.data) {
    return <InfoBar tone="warning">{chain.error}</InfoBar>;
  }
  if (!chain.data || !settings.data) {
    return <p className="muted">Loading…</p>;
  }
  const view = chain.data;
  const local = settings.data;
  if (view.state === "none") {
    return (
      <Section>
        <Row
          label="Not in a sync chain"
          hint="Create a chain on your first device, then add the others to it with its 24 words."
        >
          <Button variant="primary" onClick={() => void openWindow("setup")}>
            Set up…
          </Button>
        </Row>
      </Section>
    );
  }

  const peerById = new Map((peers.data ?? []).map((peer) => [peer.deviceId, peer]));
  const members = view.members;
  const count = members.length;

  return (
    <>
      <div className="chain-card">
        <div>
          <div className="chain-card__title">Home sync chain</div>
          <div className="muted">
            End-to-end encrypted · {count} {count === 1 ? "device" : "devices"}
            {view.fingerprint && <> · {view.fingerprint}</>}
          </div>
        </div>
        <Button
          variant="primary"
          onClick={() => onAddDeviceChange(true)}
          disabled={view.state !== "member"}
        >
          Add Device
        </Button>
      </div>

      {view.state === "waiting" && (
        <InfoBar tone="info" title="Waiting for approval.">
          On one of your other devices, open Pasta › Devices and approve this one. It shows the key{" "}
          <span className="mono selectable">{view.thisKey}</span>.
        </InfoBar>
      )}

      {view.pending.map((pending) => (
        <PendingBanner key={pending.deviceId} pending={pending} />
      ))}

      <Section title="Devices">
        {members.map((member) => (
          <DeviceRow
            key={member.deviceId}
            member={member}
            peer={peerById.get(member.deviceId)}
            thisDevice={member.thisDevice ? local.thisDevice : null}
            thisName={member.thisDevice ? local.deviceName : null}
            thisAddress={member.thisDevice ? (setup.data?.tailscale.address ?? null) : null}
            ttlChoices={local.ttlChoices}
          />
        ))}
      </Section>

      {platform === "macos" && (
        <Section
          title="iPhone and iPad"
          description="Universal Clipboard needs Handoff on and your devices nearby."
        >
          <Row
            label="Bridge iCloud Universal Clipboard"
            hint="Copies from your iPhone or iPad go to your other devices, and items from them reach your iPhone."
          >
            <Toggle
              label="Bridge iCloud Universal Clipboard"
              checked={settings.data.icloudBridge}
              disabled={bridge.busy}
              onChange={(on) => void bridge.run(() => setIcloudBridge(on))}
            />
          </Row>
          <ErrorText error={bridge.error} />
        </Section>
      )}

      <AddDeviceDialog open={addDevice} onClose={() => onAddDeviceChange(false)} />
    </>
  );
}

function PendingBanner({ pending }: { pending: PendingDevice }) {
  const action = useAction();
  return (
    <InfoBar
      tone="warning"
      title={`${pending.name} wants to join.`}
      action={
        <span className="button-row">
          <Button
            variant="primary"
            disabled={action.busy}
            onClick={() => void action.run(() => approveDevice(pending.deviceId, pending.key))}
          >
            Approve
          </Button>
          <Button
            disabled={action.busy}
            onClick={() => void action.run(() => declineDevice(pending.deviceId))}
          >
            Decline
          </Button>
        </span>
      }
    >
      {osLabel(pending.os)} on the Tailscale device “{pending.nodeName}”. Approve only if that
      device shows the key <span className="mono selectable">{pending.key}</span>.
      {pending.wasRemoved && " It was removed from the chain before."}
      <ErrorText error={action.error} />
    </InfoBar>
  );
}

function DeviceRow({
  member,
  peer,
  thisDevice,
  thisName,
  thisAddress,
  ttlChoices,
}: {
  member: ChainMember;
  peer: Peer | undefined;
  thisDevice: DeviceSettingsView | null;
  /** The name set in Rename, for this device. */
  thisName: string | null;
  thisAddress: string | null;
  ttlChoices: (number | null)[];
}) {
  const action = useAction();
  const [menu, setMenu] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [removing, setRemoving] = useState(false);
  const [name, setName] = useState("");

  // The chain record keeps the name a device joined with; show the current one:
  // this device's own setting, or what another device announced when it connected.
  const shownName = member.thisDevice ? (thisName ?? member.name) : (peer?.name ?? member.name);

  const pending = peer?.pendingSettings ?? null;
  const current = thisDevice ?? pending ?? peer?.settings ?? null;
  const connected = member.thisDevice || (peer?.connected ?? member.connected);
  const address = member.thisDevice ? thisAddress : (peer?.address ?? null);
  const latency = peer?.latencyMs;
  const change = (mode: Mode, ttlSecs: number | null) =>
    void action.run(() => setDeviceSettings(member.deviceId, mode, ttlSecs));

  return (
    <div className="device">
      <Row
        label={
          <>
            <StatusDot tone={connected ? "ok" : "offline"} />
            <span className="device__name">{shownName}</span>
            {member.thisDevice && <Badge>This device</Badge>}
            {current?.effectiveMode === "paused" && current.mode !== "paused" && (
              <Badge tone="warning">Paused</Badge>
            )}
          </>
        }
        hint={
          <>
            {osLabel(member.os)}
            {address && <> · {address}</>}
            {!member.thisDevice &&
              (connected ? latency != null && <> · {latency} ms</> : <> · offline</>)}
            {pending && <> · changes reach it when it reconnects</>}
            {!member.thisDevice && !current && <> · settings unknown until it connects</>}
          </>
        }
      >
        <Segmented
          label={`Mode of ${shownName}`}
          options={MODES}
          value={current?.mode ?? null}
          disabled={action.busy}
          size="small"
          onChange={(mode) => change(mode, current?.ttlSecs ?? null)}
        />
        <Select
          label={`Expiry on ${shownName}`}
          value={ttlValue(current?.ttlSecs ?? null)}
          options={ttlOptions(ttlChoices)}
          disabled={action.busy}
          onChange={(value) => change(current?.mode ?? "both", parseTtl(value))}
        />
        <div className="menu">
          <Button variant="plain" onClick={() => setMenu(!menu)} title="More">
            <span aria-label={`More for ${shownName}`}>⋯</span>
          </Button>
          {menu && (
            <ul className="menu__list" role="menu" onMouseLeave={() => setMenu(false)}>
              {member.thisDevice ? (
                <li>
                  <button
                    type="button"
                    role="menuitem"
                    onClick={() => {
                      setMenu(false);
                      setName(shownName);
                      setRenaming(true);
                    }}
                  >
                    Rename…
                  </button>
                </li>
              ) : (
                <li>
                  <button
                    type="button"
                    role="menuitem"
                    className="danger"
                    onClick={() => {
                      setMenu(false);
                      setRemoving(true);
                    }}
                  >
                    Remove from chain…
                  </button>
                </li>
              )}
            </ul>
          )}
        </div>
      </Row>
      <ErrorText error={action.error} />

      <Dialog
        open={renaming}
        title="Rename this device"
        onClose={() => setRenaming(false)}
        footer={
          <>
            <Button onClick={() => setRenaming(false)}>Cancel</Button>
            <Button
              variant="primary"
              onClick={() =>
                void action.run(async () => {
                  await setDeviceName(name.trim() || null);
                  setRenaming(false);
                })
              }
            >
              Rename
            </Button>
          </>
        }
      >
        <label className="field">
          <span>Name other devices see</span>
          <input value={name} onChange={(event) => setName(event.target.value)} maxLength={100} />
        </label>
        <p className="muted">
          Leave it empty to use the Tailscale name. Your other devices see the new name after Pasta
          restarts on this device.
        </p>
      </Dialog>

      <Dialog
        open={removing}
        title={`Remove ${shownName}?`}
        onClose={() => setRemoving(false)}
        footer={
          <>
            <Button onClick={() => setRemoving(false)}>Cancel</Button>
            <Button
              variant="primary"
              onClick={() =>
                void action.run(async () => {
                  await removeDevice(member.deviceId);
                  setRemoving(false);
                })
              }
            >
              Remove
            </Button>
          </>
        }
      >
        <p>
          It stops receiving items at once. To add it again later, approve it again when it asks to
          join.
        </p>
        <p className="muted">
          It still knows the 24 words. If you lost the device, start a new chain so the words stop
          working.
        </p>
      </Dialog>
    </div>
  );
}

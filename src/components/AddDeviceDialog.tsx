import { useState } from "react";
import { approveDevice, declineDevice, getChain } from "../ipc/chain";
import { EVENTS } from "../ipc/commands";
import { osLabel } from "../lib/format";
import { useAction, useResource } from "../state/useResource";
import { WordsReveal } from "./chain";
import { Button, Dialog, ErrorText, InfoBar } from "./controls";

/** Add a device to the chain (#46): install, enter the words, approve it here. */
export function AddDeviceDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const chain = useResource(getChain, [EVENTS.chain]);
  const action = useAction();
  const [approved, setApproved] = useState<string | null>(null);
  const pending = chain.data?.pending ?? [];

  const close = () => {
    setApproved(null);
    onClose();
  };

  return (
    <Dialog
      open={open}
      title="Add a device"
      onClose={close}
      wide
      footer={
        <Button variant="primary" onClick={close}>
          Done
        </Button>
      }
    >
      <ol className="steps">
        <li>
          <strong>Install Pasta on the new device</strong> and sign in to Tailscale there, with the
          same tailnet as this device. In Pasta&apos;s setup, choose <em>Join a chain</em>.
        </li>
        <li>
          <strong>Enter these 24 words on it.</strong>
          <div className="steps__body">
            <WordsReveal copyHint="Copying keeps the words out of clipboard history and off your other devices, so type them on the new device or paste them with care." />
          </div>
        </li>
        <li>
          <strong>Approve it here</strong> once it shows up, after checking that its screen shows
          the same key.
          <div className="steps__body">
            {approved && (
              <InfoBar tone="success" title={`${approved} joined.`}>
                It starts syncing right away.
              </InfoBar>
            )}
            {pending.length === 0 && !approved && (
              <p className="looking" role="status">
                <span className="spinner" aria-hidden="true" /> Looking for new devices on your
                tailnet…
              </p>
            )}
            {pending.map((device) => (
              <div key={device.deviceId} className="pending-card">
                <div>
                  <strong>{device.name}</strong>{" "}
                  <span className="muted">
                    {osLabel(device.os)} · Tailscale device “{device.nodeName}”
                  </span>
                  <div>
                    Key <span className="mono selectable">{device.key}</span>
                  </div>
                </div>
                <div className="button-row">
                  <Button
                    disabled={action.busy}
                    onClick={() => void action.run(() => declineDevice(device.deviceId))}
                  >
                    Decline
                  </Button>
                  <Button
                    variant="primary"
                    disabled={action.busy}
                    onClick={() =>
                      void action.run(async () => {
                        await approveDevice(device.deviceId, device.key);
                        setApproved(device.name);
                      })
                    }
                  >
                    Approve
                  </Button>
                </div>
              </div>
            ))}
            <ErrorText error={action.error} />
          </div>
        </li>
      </ol>
    </Dialog>
  );
}

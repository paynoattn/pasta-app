import { useState } from "react";
import { getChain, leaveChain } from "../../ipc/chain";
import { EVENTS } from "../../ipc/commands";
import { ChainChoice, WordsReveal } from "../../components/chain";
import { Button, Dialog, ErrorText, InfoBar, Row, Section } from "../../components/controls";
import { keyStoreName, platform } from "../../platform/platform";
import { useAction, useResource } from "../../state/useResource";

/** Settings › Sync chain & security (#29). */
export function ChainPage() {
  const chain = useResource(getChain, [EVENTS.chain]);
  const action = useAction();
  const [leaving, setLeaving] = useState(false);
  // Kept on screen after creating, until the new words have been seen.
  const [creating, setCreating] = useState(false);

  if (chain.error && !chain.data) return <InfoBar tone="warning">{chain.error}</InfoBar>;
  if (!chain.data) return <p className="muted">Loading…</p>;
  const view = chain.data;

  if (view.state === "none" || creating) {
    return (
      <Section
        title="Sync chain"
        description="Your devices sync only with others in the same chain."
      >
        <div className="pad">
          <ChainChoice onCreateStart={() => setCreating(true)} onDone={() => setCreating(false)} />
        </div>
      </Section>
    );
  }

  return (
    <>
      <Section title="Sync chain">
        <Row
          label="Chain"
          hint={
            view.state === "member"
              ? "This device is a member."
              : "Waiting for a member to approve this device."
          }
        >
          <span className="mono selectable">{view.fingerprint}</span>
        </Row>
        <Row
          label="This device's key"
          hint="Members see it next to this device when it asks to join."
        >
          <span className="mono selectable">{view.thisKey}</span>
        </Row>
        <div className="row row--block">
          <div className="row__text">
            <div className="row__label">The 24 words</div>
            <div className="row__hint muted">
              Add a device with them. They never leave your devices.
            </div>
            <div className="pad-top">
              <WordsReveal />
            </div>
          </div>
        </div>
      </Section>

      <Section title="Security">
        <Row label="Keys" hint={`Kept in ${keyStoreName(platform)}, never in a file.`} />
        <ul className="checklist">
          <li>Every item is encrypted on this device and decrypted only on yours.</li>
          <li>Pasta talks only over your tailnet, and checks every connection with Tailscale.</li>
          <li>A new device syncs only after one of your devices approves it.</li>
          <li>The history on this device is encrypted at rest.</li>
        </ul>
      </Section>

      <Section title="Leave">
        <Row
          label="Leave the chain"
          hint="This device stops syncing and forgets the words. Your other devices carry on."
        >
          <Button variant="danger" onClick={() => setLeaving(true)}>
            Leave…
          </Button>
        </Row>
        <ErrorText error={action.error} />
      </Section>

      <Dialog
        open={leaving}
        title="Leave the sync chain?"
        onClose={() => setLeaving(false)}
        footer={
          <>
            <Button onClick={() => setLeaving(false)}>Cancel</Button>
            <Button
              variant="primary"
              onClick={() =>
                void action.run(async () => {
                  await leaveChain();
                  setLeaving(false);
                })
              }
            >
              Leave
            </Button>
          </>
        }
      >
        <p>
          To join again, you&apos;ll need the 24 words and an approval from one of your devices.
        </p>
      </Dialog>
    </>
  );
}

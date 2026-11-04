import { useCallback, useEffect, useState } from "react";
import {
  approveDevice,
  chainWords,
  createChain,
  declineDevice,
  getChain,
  joinChain,
  leaveChain,
  onChainChanged,
  removeDevice,
  type ChainView,
} from "../ipc/chain";

/**
 * Temporary developer panel for the sync chain (#20, #22) until the real
 * setup flow (#46) and Devices screen (#43) land. Plain on purpose.
 */
export function DevChainPanel() {
  const [chain, setChain] = useState<ChainView | null>(null);
  const [words, setWords] = useState<string | null>(null);
  const [typed, setTyped] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  /** Why the first load failed, e.g. still waiting for the keychain. */
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    // Listen first: while the keychain is locked `getChain` fails, and the
    // backend sends `chain-changed` once it has started.
    const unlisten = onChainChanged(setChain);
    void unlisten.then(() =>
      getChain()
        .then(setChain)
        .catch((err: unknown) => setLoadError(String(err))),
    );
    return () => {
      void unlisten.then((stop) => stop());
    };
  }, []);

  const run = useCallback(async (action: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await action();
      setChain(await getChain());
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }, []);

  if (!chain) {
    return <section className="chain">{loadError ?? "Loading…"}</section>;
  }

  return (
    <section className="chain" aria-label="Sync chain (developer panel)">
      <h2>Sync chain</h2>
      <p className="chain__note">Developer panel until the setup flow lands.</p>
      {error && (
        <p className="chain__error" role="alert">
          {error}
        </p>
      )}

      {chain.state === "none" && (
        <>
          <p>This device isn't in a sync chain, so nothing syncs.</p>
          <button
            disabled={busy}
            onClick={() => void run(async () => setWords(await createChain()))}
          >
            Create a new chain
          </button>
          <p className="chain__warning" role="note">
            <strong>Only enter words you got from your own device.</strong> If someone sent you
            words or asked you to type them here, even someone saying they&apos;re from support,
            stop: this device would sync with theirs, and they would see everything you copy.
          </p>
          <label className="chain__join">
            Or join one with its 24 words:
            <textarea
              value={typed}
              rows={3}
              autoComplete="off"
              spellCheck={false}
              onChange={(event) => setTyped(event.target.value)}
            />
          </label>
          <button
            disabled={busy || typed.trim() === ""}
            onClick={() =>
              void run(async () => {
                await joinChain(typed);
                setTyped("");
              })
            }
          >
            Join
          </button>
        </>
      )}

      {chain.state !== "none" && (
        <p>
          Chain <strong>{chain.fingerprint}</strong> · this device's key{" "}
          <code>{chain.thisKey}</code>
        </p>
      )}

      {chain.state === "waiting" && (
        <p>
          Waiting for approval. On a device already in the chain, approve the device showing key{" "}
          <code>{chain.thisKey}</code>. Syncing starts by itself once it's approved.
        </p>
      )}

      {chain.state === "member" && (
        <>
          {chain.pending.length > 0 && (
            <>
              <h3>Waiting to join</h3>
              <ul>
                {chain.pending.map((device) => (
                  <li key={device.deviceId}>
                    {device.name} ({device.os}, Tailscale “{device.nodeName}”, key{" "}
                    <code>{device.key}</code>){device.wasRemoved && " · removed before"}{" "}
                    <button
                      disabled={busy}
                      onClick={() => void run(() => approveDevice(device.deviceId, device.key))}
                    >
                      Approve
                    </button>{" "}
                    <button
                      disabled={busy}
                      onClick={() => void run(() => declineDevice(device.deviceId))}
                    >
                      Decline
                    </button>
                  </li>
                ))}
              </ul>
            </>
          )}
          <h3>Members</h3>
          <ul>
            {chain.members.map((member) => (
              <li key={member.deviceId}>
                {member.connected ? "●" : "○"} {member.name} ({member.os})
                {member.thisDevice ? (
                  " · this device"
                ) : (
                  <>
                    {" "}
                    <button
                      disabled={busy}
                      onClick={() => void run(() => removeDevice(member.deviceId))}
                    >
                      Remove
                    </button>
                  </>
                )}
              </li>
            ))}
          </ul>
          <button
            disabled={busy}
            onClick={() => void run(async () => setWords(words ? null : await chainWords()))}
          >
            {words ? "Hide the words" : "Show the 24 words"}
          </button>
        </>
      )}

      {words && (
        <div className="chain__words">
          <p className="chain__warning" role="note">
            On your other device, open Pasta&apos;s sync chain panel and type these words under
            &ldquo;join one with its 24 words&rdquo;.{" "}
            <strong>
              Treat them like a password. Never give them to anyone, even someone saying
              they&apos;re from support.
            </strong>{" "}
            With the words, someone could join your chain or take over a device while it joins, and
            see what you copy.
          </p>
          <p className="chain__code">{words}</p>
        </div>
      )}

      {chain.state !== "none" && (
        <p>
          <button
            disabled={busy}
            onClick={() =>
              void run(async () => {
                await leaveChain();
                setWords(null);
              })
            }
          >
            Leave the chain
          </button>
        </p>
      )}
    </section>
  );
}

import { useCallback, useEffect, useState } from "react";
import {
  acceptApproval,
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
        <>
          <p>
            Waiting for approval. On a device already in the chain, approve the device showing key{" "}
            <code>{chain.thisKey}</code>.
          </p>
          {chain.offers.length > 0 && (
            <>
              <h3>Approved by</h3>
              <p className="chain__note">
                Accept only if the key matches the one that device shows as its own.
              </p>
              <ul>
                {chain.offers.map((offer) => (
                  <li key={`${offer.approverKey}-${offer.viaNodeName}-${offer.approverName}`}>
                    {offer.approverName} ({offer.approverOs}, Tailscale “{offer.viaNodeName}”, key{" "}
                    <code>{offer.approverKey}</code>){" "}
                    <button
                      disabled={busy}
                      onClick={() => void run(() => acceptApproval(offer.approverKey))}
                    >
                      Accept
                    </button>
                  </li>
                ))}
              </ul>
            </>
          )}
        </>
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
        <p className="chain__words">
          {words}
          <br />
          <small>Type these on the other device. Anyone with them can ask to join.</small>
        </p>
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

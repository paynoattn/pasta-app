import { useState } from "react";
import { chainQr, chainWords, copyChainWords, createChain, joinChain } from "../ipc/chain";
import { Button, ErrorText, InfoBar } from "./controls";
import { useAction } from "../state/useResource";

/** The 24 words as a numbered grid. */
export function WordGrid({ words }: { words: string }) {
  return (
    <ol className="words selectable" aria-label="The 24 words">
      {words
        .split(/\s+/)
        .filter(Boolean)
        .map((word, index) => (
          <li key={index}>{word}</li>
        ))}
    </ol>
  );
}

/** Why the words matter, shown before and next to them (#22, #46). */
export function WordsWarning() {
  return (
    <InfoBar tone="warning" title="The words are the encryption key.">
      Treat them like a password. Never give them to anyone, even someone saying they&apos;re from
      support. With the words, someone on your tailnet could join your chain, or take over a device
      while it joins, and see what you copy.
    </InfoBar>
  );
}

/** The words, QR code and "Copy words", behind a confirmation (#29, #46). */
export function WordsReveal({ copyHint }: { copyHint?: string }) {
  const [stage, setStage] = useState<"hidden" | "confirm" | "shown">("hidden");
  const [words, setWords] = useState<string | null>(null);
  const [qr, setQr] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const action = useAction();

  if (stage === "hidden") {
    return (
      <Button onClick={() => setStage("confirm")} variant="default">
        Show the 24 words…
      </Button>
    );
  }
  if (stage === "confirm") {
    return (
      <div className="reveal">
        <WordsWarning />
        <p>Make sure nobody can see your screen.</p>
        <div className="button-row">
          <Button onClick={() => setStage("hidden")}>Cancel</Button>
          <Button
            variant="primary"
            disabled={action.busy}
            onClick={() =>
              void action.run(async () => {
                const [loaded, code] = await Promise.all([chainWords(), chainQr()]);
                setWords(loaded);
                setQr(code);
                setStage("shown");
              })
            }
          >
            Show the words
          </Button>
        </div>
        <ErrorText error={action.error} />
      </div>
    );
  }
  return (
    <div className="reveal">
      <div className="reveal__words">
        {words && <WordGrid words={words} />}
        {qr && (
          <img
            className="reveal__qr"
            src={qr}
            alt="QR code of the 24 words"
            width={160}
            height={160}
          />
        )}
      </div>
      <div className="button-row">
        <Button
          onClick={() =>
            void action.run(async () => {
              await copyChainWords();
              setCopied(true);
            })
          }
        >
          {copied ? "Copied" : "Copy words"}
        </Button>
        <Button
          onClick={() => {
            setStage("hidden");
            setWords(null);
            setQr(null);
            setCopied(false);
          }}
        >
          Hide
        </Button>
      </div>
      <p className="muted">
        {copyHint ??
          "Copying keeps the words out of clipboard history and off your other devices. Clear the clipboard once you've pasted them."}
      </p>
      <ErrorText error={action.error} />
    </div>
  );
}

/**
 * Create a chain or join one with its words (#20, #43).
 *
 * Creating a chain flips the chain state to `member` before the new words are
 * shown, so a parent that renders this only while there's no chain must keep
 * it mounted from `onCreateStart` until `onDone`. `continueButton={false}` leaves
 * the Continue under the new words to the parent, e.g. the setup flow's own.
 */
export function ChainChoice({
  onCreateStart,
  onDone,
  continueButton = true,
}: {
  onCreateStart?: () => void;
  onDone?: () => void;
  continueButton?: boolean;
}) {
  const [mode, setMode] = useState<"choose" | "create" | "join">("choose");
  const [created, setCreated] = useState<string | null>(null);
  const [typed, setTyped] = useState("");
  const action = useAction();

  if (mode === "create") {
    return (
      <div className="chain-choice">
        {created ? (
          <>
            <p>
              Your chain is ready. On your next device, choose <strong>Join a chain</strong> and
              type these words.
            </p>
            <WordsWarning />
            <WordGrid words={created} />
            <p className="muted">
              You can show them again later in Settings › Sync chain &amp; security.
            </p>
            {continueButton && (
              <div className="button-row">
                <Button variant="primary" onClick={onDone}>
                  Continue
                </Button>
              </div>
            )}
          </>
        ) : (
          <>
            <p>
              This device becomes the first member. Pasta makes 24 words; every device you add uses
              them to join, and their key encrypts everything you sync.
            </p>
            <div className="button-row">
              <Button onClick={() => setMode("choose")}>Back</Button>
              <Button
                variant="primary"
                disabled={action.busy}
                onClick={() =>
                  void action.run(async () => {
                    onCreateStart?.();
                    setCreated(await createChain());
                  })
                }
              >
                Create the chain
              </Button>
            </div>
          </>
        )}
        <ErrorText error={action.error} />
      </div>
    );
  }
  if (mode === "join") {
    return (
      <div className="chain-choice">
        <InfoBar tone="warning" title="Only enter words you got from your own device.">
          If someone sent you words or asked you to type them here, even someone saying they&apos;re
          from support, stop: this device would sync with theirs, and they would see everything you
          copy.
        </InfoBar>
        <label className="field">
          <span>The 24 words, from Settings › Devices › Add Device on your other device</span>
          <textarea
            className="selectable"
            value={typed}
            rows={4}
            autoComplete="off"
            spellCheck={false}
            onChange={(event) => setTyped(event.target.value)}
          />
        </label>
        <div className="button-row">
          <Button onClick={() => setMode("choose")}>Back</Button>
          <Button
            variant="primary"
            disabled={action.busy || typed.trim() === ""}
            onClick={() =>
              void action.run(async () => {
                await joinChain(typed);
                setTyped("");
                onDone?.();
              })
            }
          >
            Join
          </Button>
        </div>
        <ErrorText error={action.error} />
      </div>
    );
  }
  return (
    <div className="chain-choice chain-choice--options">
      <button type="button" className="choice-card" onClick={() => setMode("create")}>
        <strong>Create a new chain</strong>
        <span className="muted">This is your first device with Pasta.</span>
      </button>
      <button type="button" className="choice-card" onClick={() => setMode("join")}>
        <strong>Join a chain</strong>
        <span className="muted">
          Another of your devices already runs Pasta. You&apos;ll need its 24 words.
        </span>
      </button>
    </div>
  );
}

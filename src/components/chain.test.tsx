import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as chain from "../ipc/chain";
import { ChainChoice, WordsReveal } from "./chain";

vi.mock("../ipc/chain", () => ({
  chainQr: vi.fn(),
  chainWords: vi.fn(),
  copyChainWords: vi.fn(),
  createChain: vi.fn(),
  joinChain: vi.fn(),
}));

const chainWords = vi.mocked(chain.chainWords);
const chainQr = vi.mocked(chain.chainQr);
const copyChainWords = vi.mocked(chain.copyChainWords);
const createChain = vi.mocked(chain.createChain);
const joinChain = vi.mocked(chain.joinChain);

beforeEach(() => {
  chainWords.mockReset();
  chainQr.mockReset();
  copyChainWords.mockReset();
  createChain.mockReset();
  joinChain.mockReset();
});

afterEach(cleanup);

const WORDS = Array.from({ length: 24 }, (_, i) => `word${i + 1}`).join(" ");

describe("WordsReveal", () => {
  it("loads the words and QR only after the confirmation", async () => {
    const user = userEvent.setup();
    chainWords.mockResolvedValue(WORDS);
    chainQr.mockResolvedValue("data:image/svg+xml;base64,AAAA");
    render(<WordsReveal />);

    // Hidden: only the entry button; nothing is loaded.
    expect(screen.getByRole("button", { name: "Show the 24 words…" })).toBeInstanceOf(HTMLElement);
    expect(chainWords).not.toHaveBeenCalled();

    // Confirm stage: the warning shows, but the words still aren't asked for.
    await user.click(screen.getByRole("button", { name: "Show the 24 words…" }));
    expect(screen.getByText(/the words are the encryption key/i)).toBeInstanceOf(HTMLElement);
    expect(chainWords).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "Show the words" }));
    await waitFor(() =>
      expect(screen.getByRole("list", { name: "The 24 words" })).toBeInstanceOf(HTMLElement),
    );
    expect(chainWords).toHaveBeenCalledExactlyOnceWith();
    expect(chainQr).toHaveBeenCalledExactlyOnceWith();
    expect(screen.getAllByRole("listitem")).toHaveLength(24);
    expect(screen.getByAltText("QR code of the 24 words")).toBeInstanceOf(HTMLElement);
  });

  it("copying the words calls copyChainWords and flips the label", async () => {
    const user = userEvent.setup();
    chainWords.mockResolvedValue(WORDS);
    chainQr.mockResolvedValue("data:image/svg+xml;base64,AAAA");
    copyChainWords.mockResolvedValue(undefined);
    render(<WordsReveal />);

    await user.click(screen.getByRole("button", { name: "Show the 24 words…" }));
    await user.click(screen.getByRole("button", { name: "Show the words" }));
    await user.click(await screen.findByRole("button", { name: "Copy words" }));

    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Copied" })).toBeInstanceOf(HTMLElement),
    );
    expect(copyChainWords).toHaveBeenCalledExactlyOnceWith();
    // Copying never loads the words again.
    expect(chainWords).toHaveBeenCalledTimes(1);
  });

  it("shows the backend's error instead of the words when loading fails", async () => {
    const user = userEvent.setup();
    chainWords.mockRejectedValue("the keychain is locked");
    chainQr.mockResolvedValue("data:image/svg+xml;base64,AAAA");
    render(<WordsReveal />);

    await user.click(screen.getByRole("button", { name: "Show the 24 words…" }));
    await user.click(screen.getByRole("button", { name: "Show the words" }));

    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("the keychain is locked");
    expect(screen.queryByRole("list", { name: "The 24 words" })).toBeNull();
  });
});

describe("ChainChoice", () => {
  it("creates a chain and shows its words", async () => {
    const user = userEvent.setup();
    const words = Array.from({ length: 24 }, (_, i) => `w${i}`).join(" ");
    createChain.mockResolvedValue(words);
    const onDone = vi.fn();
    render(<ChainChoice onDone={onDone} />);

    await user.click(screen.getByRole("button", { name: /Create a new chain/i }));
    await user.click(screen.getByRole("button", { name: "Create the chain" }));
    expect(await screen.findAllByRole("listitem")).toHaveLength(24);
    expect(createChain).toHaveBeenCalledExactlyOnceWith();
    expect(onDone).not.toHaveBeenCalled();

    // Button passes the click through, so `onDone` runs once (with the event).
    await user.click(screen.getByRole("button", { name: "Continue" }));
    expect(onDone).toHaveBeenCalledTimes(1);
  });

  it("leaves the Continue under new words to the parent when asked", async () => {
    const user = userEvent.setup();
    createChain.mockResolvedValue(WORDS);
    const onCreateStart = vi.fn();
    render(<ChainChoice onCreateStart={onCreateStart} continueButton={false} />);

    await user.click(screen.getByRole("button", { name: /Create a new chain/i }));
    await user.click(screen.getByRole("button", { name: "Create the chain" }));

    await screen.findByRole("list", { name: "The 24 words" });
    expect(onCreateStart).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("button", { name: "Continue" })).toBeNull();
  });

  it("joining needs the words and sends exactly what was typed", async () => {
    const user = userEvent.setup();
    joinChain.mockResolvedValue(undefined);
    const onDone = vi.fn();
    render(<ChainChoice onDone={onDone} />);

    await user.click(screen.getByRole("button", { name: /Join a chain/i }));
    const join = screen.getByRole("button", { name: "Join" }) as HTMLButtonElement;
    expect(join.disabled).toBe(true);

    await user.type(screen.getByRole("textbox"), "   ");
    // Blank words don't count.
    expect(join.disabled).toBe(true);

    await user.type(screen.getByRole("textbox"), "alpha bravo charlie");
    expect(join.disabled).toBe(false);
    await user.click(join);

    await waitFor(() =>
      expect(joinChain).toHaveBeenCalledExactlyOnceWith("   alpha bravo charlie"),
    );
    await waitFor(() => expect(onDone).toHaveBeenCalledTimes(1));
  });

  it("shows the backend's rejection when the words are wrong", async () => {
    const user = userEvent.setup();
    joinChain.mockRejectedValue("those words aren't a valid chain code");
    render(<ChainChoice />);

    await user.click(screen.getByRole("button", { name: /Join a chain/i }));
    await user.type(screen.getByRole("textbox"), "nonsense");
    await user.click(screen.getByRole("button", { name: "Join" }));

    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toContain("those words aren't a valid chain code");
  });
});

import { act, cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as chainIpc from "../../ipc/chain";
import * as commands from "../../ipc/commands";
import type { ChainView } from "../../ipc/bindings/ChainView";
import { ChainPage } from "./ChainPage";

vi.mock("../../ipc/chain", () => ({
  chainQr: vi.fn(),
  chainWords: vi.fn(),
  copyChainWords: vi.fn(),
  createChain: vi.fn(),
  getChain: vi.fn(),
  joinChain: vi.fn(),
  leaveChain: vi.fn(),
}));
vi.mock("../../ipc/commands", () => ({
  EVENTS: { chain: "chain-changed" },
  on: vi.fn(),
}));
vi.mock("../../platform/platform", () => ({
  platform: "linux",
  keyStoreName: vi.fn(() => "the Secret Service (KWallet)"),
}));

const getChain = vi.mocked(chainIpc.getChain);
const createChain = vi.mocked(chainIpc.createChain);
const on = vi.mocked(commands.on);

const NONE: ChainView = {
  state: "none",
  fingerprint: null,
  thisKey: "a1b2c3d4",
  members: [],
  pending: [],
};
const MEMBER: ChainView = { ...NONE, state: "member", fingerprint: "ABCD-EFGH" };
const WORDS = Array.from({ length: 24 }, (_, i) => `word${i + 1}`).join(" ");

let fireChainChanged: () => void = () => {};

beforeEach(() => {
  getChain.mockReset();
  createChain.mockReset();
  on.mockReset();
  on.mockImplementation((_event, handler) => {
    fireChainChanged = () => handler(undefined as never);
    return Promise.resolve(() => {});
  });
});

afterEach(cleanup);

describe("ChainPage", () => {
  it("keeps the new words on screen when creating flips the chain to member", async () => {
    const user = userEvent.setup();
    getChain.mockResolvedValue(NONE);
    // Like Rust: the chain-changed event goes out before the command returns.
    createChain.mockImplementation(async () => {
      getChain.mockResolvedValue(MEMBER);
      fireChainChanged();
      await new Promise((resolve) => setTimeout(resolve, 0));
      return WORDS;
    });
    render(<ChainPage />);

    await user.click(await screen.findByRole("button", { name: /Create a new chain/i }));
    await user.click(screen.getByRole("button", { name: "Create the chain" }));

    const grid = await screen.findByRole("list", { name: "The 24 words" });
    expect(grid.querySelectorAll("li")).toHaveLength(24);
    // The member page loaded underneath, but waits until the words were seen.
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 10));
    });
    expect(screen.queryByRole("button", { name: "Leave…" })).toBeNull();

    await user.click(screen.getByRole("button", { name: "Continue" }));
    expect(await screen.findByRole("button", { name: "Leave…" })).toBeInstanceOf(HTMLElement);
    expect(screen.queryByRole("list", { name: "The 24 words" })).toBeNull();
  });
});

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  approveDevice,
  chainWords,
  declineDevice,
  getChain,
  onChainChanged,
  type ChainView,
} from "../ipc/chain";
import { DevChainPanel } from "./DevChainPanel";

vi.mock("../ipc/chain", () => ({
  approveDevice: vi.fn(),
  chainWords: vi.fn(),
  createChain: vi.fn(),
  declineDevice: vi.fn(),
  getChain: vi.fn(),
  joinChain: vi.fn(),
  leaveChain: vi.fn(),
  onChainChanged: vi.fn(),
  removeDevice: vi.fn(),
}));

const THIS_KEY = "a1b2c3d4e5f60718";
const PENDING_KEY = "1122334455667788";
const PENDING_ID = "00000000-0000-4000-8000-000000000002";

const waiting: ChainView = {
  state: "waiting",
  fingerprint: "ABCD-EFGH",
  thisKey: THIS_KEY,
  members: [],
  pending: [],
};

const member: ChainView = {
  state: "member",
  fingerprint: "ABCD-EFGH",
  thisKey: THIS_KEY,
  members: [
    {
      deviceId: "00000000-0000-4000-8000-000000000001",
      name: "macbook-pro",
      os: "macos",
      thisDevice: true,
      connected: true,
    },
  ],
  pending: [
    {
      deviceId: PENDING_ID,
      name: "win-workstation",
      os: "windows",
      nodeName: "win-workstation-ts",
      key: PENDING_KEY,
      wasRemoved: false,
    },
  ],
};

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  vi.mocked(approveDevice).mockReset().mockResolvedValue(undefined);
  vi.mocked(declineDevice).mockReset().mockResolvedValue(undefined);
  vi.mocked(getChain).mockReset();
  vi.mocked(onChainChanged)
    .mockReset()
    .mockResolvedValue(() => undefined);
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

async function render(view: ChainView) {
  vi.mocked(getChain).mockResolvedValue(view);
  await act(async () => {
    root.render(<DevChainPanel />);
  });
}

function button(label: string): HTMLButtonElement {
  const found = [...container.querySelectorAll("button")].find((el) => el.textContent === label);
  if (!found) throw new Error(`no "${label}" button in: ${container.textContent}`);
  return found;
}

async function click(el: HTMLElement) {
  await act(async () => {
    el.click();
  });
}

describe("DevChainPanel on a device waiting for approval", () => {
  it("shows the key to approve and has nothing to accept here", async () => {
    await render(waiting);

    expect(container.textContent).toContain("Waiting for approval");
    expect(container.textContent).toContain(THIS_KEY);
    expect(container.textContent).toContain("Syncing starts by itself");
    expect(container.querySelectorAll("button").length).toBe(1); // only "Leave the chain"
  });
});

describe("DevChainPanel on a member", () => {
  it("approves a pending device by the key it shows, not just its id", async () => {
    await render(member);

    expect(container.textContent).toContain("win-workstation (windows");
    expect(container.textContent).toContain(PENDING_KEY);
    await click(button("Approve"));
    expect(approveDevice).toHaveBeenCalledExactlyOnceWith(PENDING_ID, PENDING_KEY);
    expect(declineDevice).not.toHaveBeenCalled();
  });

  it("declines by id and surfaces a failed approval", async () => {
    vi.mocked(approveDevice).mockRejectedValue("that device's key changed; compare it again");
    await render(member);

    await click(button("Approve"));
    expect(container.querySelector('[role="alert"]')?.textContent).toContain("key changed");
    await click(button("Decline"));
    expect(declineDevice).toHaveBeenCalledExactlyOnceWith(PENDING_ID);
  });
});

describe("DevChainPanel while Pasta is starting", () => {
  it("shows the chain once the backend announces it", async () => {
    let announce: ((chain: ChainView) => void) | undefined;
    vi.mocked(onChainChanged).mockImplementation((handler) => {
      announce = handler;
      return Promise.resolve(() => undefined);
    });
    vi.mocked(getChain).mockRejectedValue("Pasta is still starting (is the keychain locked?)");
    await act(async () => {
      root.render(<DevChainPanel />);
    });
    expect(container.textContent).toContain("still starting");

    await act(async () => {
      announce?.({ ...member, pending: [] });
    });
    expect(container.textContent).toContain("ABCD-EFGH");
    expect(container.textContent).not.toContain("still starting");
  });
});

describe("DevChainPanel warnings about the words", () => {
  it("warns before joining with words someone else gave you", async () => {
    await render({ ...waiting, state: "none", fingerprint: null });

    const note = container.querySelector('[role="note"]')?.textContent;
    expect(note).toContain("Only enter words you got from your own device");
    expect(note).toContain("see everything you copy");
  });

  it("shows the words with a warning to treat them like a password", async () => {
    vi.mocked(chainWords).mockResolvedValue("abandon ability able");
    await render({ ...member, pending: [] });

    await click(button("Show the 24 words"));
    const shown = container.querySelector(".chain__words")?.textContent;
    expect(shown).toContain("abandon ability able");
    expect(shown).toContain("Treat them like a password");
  });
});

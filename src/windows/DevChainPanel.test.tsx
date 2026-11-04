import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  acceptApproval,
  approveDevice,
  declineDevice,
  getChain,
  onChainChanged,
  type ChainView,
} from "../ipc/chain";
import { DevChainPanel } from "./DevChainPanel";

vi.mock("../ipc/chain", () => ({
  acceptApproval: vi.fn(),
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
const APPROVER_KEY = "0f1e2d3c4b5a6978";
const PENDING_KEY = "1122334455667788";
const PENDING_ID = "00000000-0000-4000-8000-000000000002";

const waiting: ChainView = {
  state: "waiting",
  fingerprint: "ABCD-EFGH",
  thisKey: THIS_KEY,
  members: [],
  pending: [],
  offers: [
    {
      approverName: "macbook-pro",
      approverOs: "macos",
      approverKey: APPROVER_KEY,
      viaNodeName: "macbook-pro-ts",
    },
  ],
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
  offers: [],
};

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  vi.mocked(acceptApproval).mockReset().mockResolvedValue(undefined);
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
  it("shows who approved it, with their key, and accepts by that key", async () => {
    await render(waiting);

    const text = container.textContent;
    expect(text).toContain("Waiting for approval");
    expect(text).toContain(THIS_KEY);
    expect(text).toContain("macbook-pro (macos");
    expect(text).toContain("macbook-pro-ts");
    expect(text).toContain(APPROVER_KEY);

    await click(button("Accept"));
    expect(acceptApproval).toHaveBeenCalledExactlyOnceWith(APPROVER_KEY);
    // The panel refreshes from the backend afterwards.
    expect(getChain).toHaveBeenCalledTimes(2);
  });

  it("offers nothing to accept until a member has approved it", async () => {
    await render({ ...waiting, offers: [] });

    expect(container.textContent).toContain("Waiting for approval");
    expect(container.textContent).not.toContain("Approved by");
    expect(container.querySelectorAll("button").length).toBe(1); // only "Leave the chain"
  });

  it("shows why an approval was refused", async () => {
    vi.mocked(acceptApproval).mockRejectedValue("no approval from a device with that key");
    await render(waiting);

    await click(button("Accept"));
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "no approval from a device with that key",
    );
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

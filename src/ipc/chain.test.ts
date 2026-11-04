import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { beforeEach, describe, expect, it, vi } from "vitest";
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
} from "./chain";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const DEVICE_ID = "00000000-0000-4000-8000-000000000002";

const chain: ChainView = {
  state: "member",
  fingerprint: "ABCD-EFGH",
  thisKey: "a1b2c3d4e5f60718",
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
      deviceId: DEVICE_ID,
      name: "win-workstation",
      os: "windows",
      nodeName: "win-workstation",
      key: "0f1e2d3c4b5a6978",
      wasRemoved: false,
    },
  ],
};

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(listen).mockReset();
});

describe("getChain", () => {
  it("calls chain_status without arguments and returns the view", async () => {
    vi.mocked(invoke).mockResolvedValue(chain);

    await expect(getChain()).resolves.toEqual(chain);
    expect(invoke).toHaveBeenCalledExactlyOnceWith("chain_status");
  });
});

describe("createChain", () => {
  it("calls create_chain and resolves to the words", async () => {
    vi.mocked(invoke).mockResolvedValue("word ".repeat(23) + "word");

    await expect(createChain()).resolves.toBe("word ".repeat(23) + "word");
    expect(invoke).toHaveBeenCalledExactlyOnceWith("create_chain");
  });
});

describe("joinChain", () => {
  it("passes the typed words as `words`", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);

    await expect(joinChain("alpha bravo charlie")).resolves.toBeUndefined();
    expect(invoke).toHaveBeenCalledExactlyOnceWith("join_chain", { words: "alpha bravo charlie" });
  });

  it("surfaces the backend's error", async () => {
    vi.mocked(invoke).mockRejectedValue("those words aren't a valid chain code");

    await expect(joinChain("nonsense")).rejects.toBe("those words aren't a valid chain code");
  });
});

describe("leaveChain", () => {
  it("calls leave_chain without arguments", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);

    await expect(leaveChain()).resolves.toBeUndefined();
    expect(invoke).toHaveBeenCalledExactlyOnceWith("leave_chain");
  });
});

describe("chainWords", () => {
  it("calls chain_words and resolves to the words", async () => {
    vi.mocked(invoke).mockResolvedValue("one two three");

    await expect(chainWords()).resolves.toBe("one two three");
    expect(invoke).toHaveBeenCalledExactlyOnceWith("chain_words");
  });
});

describe("approveDevice", () => {
  it("calls approve_device with the device id and the key the user compared", async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);

    await expect(approveDevice(DEVICE_ID, "0f1e2d3c4b5a6978")).resolves.toBeUndefined();
    expect(invoke).toHaveBeenCalledExactlyOnceWith("approve_device", {
      deviceId: DEVICE_ID,
      key: "0f1e2d3c4b5a6978",
    });
  });
});

describe.each([
  ["declineDevice", declineDevice, "decline_device"],
  ["removeDevice", removeDevice, "remove_device"],
] as const)("%s", (_name, fn, command) => {
  it(`calls ${command} with the device id as \`deviceId\``, async () => {
    vi.mocked(invoke).mockResolvedValue(undefined);

    await expect(fn(DEVICE_ID)).resolves.toBeUndefined();
    expect(invoke).toHaveBeenCalledExactlyOnceWith(command, { deviceId: DEVICE_ID });
  });

  it("surfaces the backend's error", async () => {
    vi.mocked(invoke).mockRejectedValue("that device isn't waiting for approval");

    await expect(fn(DEVICE_ID)).rejects.toBe("that device isn't waiting for approval");
  });
});

describe("onChainChanged", () => {
  it("passes each chain-changed payload to the handler and returns the unlisten function", async () => {
    const unlisten = vi.fn();
    vi.mocked(listen).mockImplementation((_event, callback) => {
      callback({ event: "chain-changed", id: 1, payload: chain });
      callback({
        event: "chain-changed",
        id: 2,
        payload: { ...chain, state: "none", fingerprint: null },
      });
      return Promise.resolve(unlisten);
    });
    const handler = vi.fn();

    await expect(onChainChanged(handler)).resolves.toBe(unlisten);
    expect(listen).toHaveBeenCalledExactlyOnceWith("chain-changed", expect.any(Function));
    expect(handler).toHaveBeenCalledTimes(2);
    expect(handler).toHaveBeenNthCalledWith(1, chain);
    expect(handler).toHaveBeenNthCalledWith(2, { ...chain, state: "none", fingerprint: null });
  });
});

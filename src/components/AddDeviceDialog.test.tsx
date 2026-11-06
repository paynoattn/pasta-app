import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as chainIpc from "../ipc/chain";
import * as commands from "../ipc/commands";
import { AddDeviceDialog } from "./AddDeviceDialog";

vi.mock("../ipc/chain", () => ({
  approveDevice: vi.fn(),
  declineDevice: vi.fn(),
  getChain: vi.fn(),
}));
vi.mock("../ipc/commands", () => ({
  EVENTS: { chain: "chain-changed" },
  on: vi.fn().mockResolvedValue(() => {}),
}));

const getChain = vi.mocked(chainIpc.getChain);
const approveDevice = vi.mocked(chainIpc.approveDevice);
const declineDevice = vi.mocked(chainIpc.declineDevice);
const on = vi.mocked(commands.on);

function chainView(pending: Parameters<typeof getChain> extends never ? never : unknown[]) {
  return {
    state: "member",
    fingerprint: "ABCD-EFGH",
    thisKey: "a1b2c3d4e5f60718",
    members: [],
    pending,
  };
}

beforeEach(() => {
  getChain.mockReset();
  approveDevice.mockReset();
  declineDevice.mockReset();
  on.mockClear();
});

afterEach(cleanup);

describe("AddDeviceDialog", () => {
  it("shows that it is looking while no device is waiting", async () => {
    getChain.mockResolvedValue(chainView([]) as never);
    render(<AddDeviceDialog open={true} onClose={() => {}} />);

    const status = await screen.findByRole("status");
    expect(status.textContent).toContain("Looking for new devices on your tailnet");
    expect(approveDevice).not.toHaveBeenCalled();
  });

  it("lists a waiting device and approves it with the key shown on its screen", async () => {
    const user = userEvent.setup();
    getChain.mockResolvedValue(
      chainView([
        {
          deviceId: "00000000-0000-4000-8000-000000000002",
          name: "win-workstation",
          os: "windows",
          nodeName: "win-workstation",
          key: "0f1e2d3c4b5a6978",
          wasRemoved: false,
        },
      ]) as never,
    );
    approveDevice.mockResolvedValue(undefined);
    const onClose = vi.fn();
    render(<AddDeviceDialog open={true} onClose={onClose} />);

    expect(await screen.findByText("win-workstation")).toBeInstanceOf(HTMLElement);
    expect(screen.getByText("0f1e2d3c4b5a6978")).toBeInstanceOf(HTMLElement);
    expect(screen.queryByRole("status")).toBeNull();

    await user.click(screen.getByRole("button", { name: "Approve" }));
    await waitFor(() =>
      expect(approveDevice).toHaveBeenCalledExactlyOnceWith(
        "00000000-0000-4000-8000-000000000002",
        "0f1e2d3c4b5a6978",
      ),
    );
    expect(declineDevice).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
    // The success note names the device that joined.
    expect(await screen.findByText("win-workstation joined.")).toBeInstanceOf(HTMLElement);
  });

  it("declining asks the backend without closing the dialog", async () => {
    const user = userEvent.setup();
    getChain.mockResolvedValue(
      chainView([
        {
          deviceId: "00000000-0000-4000-8000-000000000003",
          name: "cachy-desktop",
          os: "linux",
          nodeName: "cachy-desktop",
          key: "1122334455667788",
          wasRemoved: false,
        },
      ]) as never,
    );
    declineDevice.mockResolvedValue(undefined);
    const onClose = vi.fn();
    render(<AddDeviceDialog open={true} onClose={onClose} />);

    await user.click(await screen.findByRole("button", { name: "Decline" }));
    await waitFor(() =>
      expect(declineDevice).toHaveBeenCalledExactlyOnceWith("00000000-0000-4000-8000-000000000003"),
    );
    expect(approveDevice).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
  });
});

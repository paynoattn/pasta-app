import { invoke } from "@tauri-apps/api/core";
import { describe, expect, it, vi } from "vitest";
import { getAppInfo } from "./commands";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

describe("getAppInfo", () => {
  it("calls the app_info command and returns its result", async () => {
    vi.mocked(invoke).mockResolvedValue({ version: "0.1.0", protocolVersion: 1 });

    await expect(getAppInfo()).resolves.toEqual({ version: "0.1.0", protocolVersion: 1 });
    expect(invoke).toHaveBeenCalledWith("app_info");
  });
});

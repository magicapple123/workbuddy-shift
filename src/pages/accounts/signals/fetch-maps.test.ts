import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/lib/api", () => ({
  isWebui: vi.fn(),
  isDemoMode: vi.fn(() => false),
  httpCall: vi.fn(),
  getTravelStatus: vi.fn(),
}));

import * as api from "@/lib/api";
import { fetchTravelMap } from "./fetch-maps";

const status = () => ({ label: "traveling" as const, rewardCredit: 10, locationName: "巴黎", arriveAt: null });

describe("fetchTravelMap", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("webui 多账号只请求一次批量接口，并按账号过滤", async () => {
    vi.mocked(api.isWebui).mockReturnValue(true);
    vi.mocked(api.isDemoMode).mockReturnValue(false);
    vi.mocked(api.httpCall).mockResolvedValue({
      accounts: [
        { accountId: "a", ...status() },
        { accountId: "b", ...status() },
        { accountId: "not-visible", ...status() },
      ],
    });

    const result = await fetchTravelMap(["a", "b"]);

    expect(api.httpCall).toHaveBeenCalledTimes(1);
    expect(api.httpCall).toHaveBeenCalledWith("get_travel_status");
    expect(api.getTravelStatus).not.toHaveBeenCalled();
    expect(Object.keys(result).sort()).toEqual(["a", "b"]);
  });

  it("webui 批量接口失败时返回空映射（调用方保留旧值）", async () => {
    vi.mocked(api.isWebui).mockReturnValue(true);
    vi.mocked(api.isDemoMode).mockReturnValue(false);
    vi.mocked(api.httpCall).mockRejectedValue(new Error("boom"));

    await expect(fetchTravelMap(["a", "b"])).resolves.toEqual({});
  });

  it("演示模式不请求批量 HTTP，仍走逐账号演示数据", async () => {
    vi.mocked(api.isWebui).mockReturnValue(true);
    vi.mocked(api.isDemoMode).mockReturnValue(true);
    vi.mocked(api.getTravelStatus).mockResolvedValue(status());

    const result = await fetchTravelMap(["a", "b"]);

    expect(api.httpCall).not.toHaveBeenCalled();
    expect(api.getTravelStatus).toHaveBeenCalledTimes(2);
    expect(Object.keys(result).sort()).toEqual(["a", "b"]);
  });

  it("桌面端逐账号请求，失败账号不写入", async () => {
    vi.mocked(api.isWebui).mockReturnValue(false);
    vi.mocked(api.getTravelStatus).mockImplementation(async (id: string) => {
      if (id === "b") throw new Error("boom");
      return status();
    });

    const result = await fetchTravelMap(["a", "b"]);

    expect(api.httpCall).not.toHaveBeenCalled();
    expect(api.getTravelStatus).toHaveBeenCalledTimes(2);
    expect(Object.keys(result)).toEqual(["a"]);
  });
});

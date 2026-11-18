import { beforeEach, describe, expect, it, vi } from "vitest";

const getCreditStatistics = vi.fn();
const refreshOfficialUsage = vi.fn();

vi.mock("@/lib/api", () => ({
  getCreditStatistics: (...args: unknown[]) => getCreditStatistics(...args),
  refreshOfficialUsage: (...args: unknown[]) => refreshOfficialUsage(...args),
}));

import type { CreditStatistics } from "@/lib/types";
import { runProgressiveRefresh } from "./progressive-refresh";

function stats(marker: string): CreditStatistics {
  return { generatedAt: 1, marker } as unknown as CreditStatistics;
}

describe("积分统计渐进式刷新", () => {
  beforeEach(() => {
    getCreditStatistics.mockReset();
    refreshOfficialUsage.mockReset();
  });

  it("先返回本地数字，再并行补积分与官方用量", async () => {
    const order: string[] = [];
    let releaseCredits: (() => void) | null = null;
    let releaseOfficial: (() => void) | null = null;

    getCreditStatistics.mockImplementation(async () => {
      order.push("local");
      return stats("local");
    });
    refreshOfficialUsage.mockImplementation(
      () =>
        new Promise((resolve) => {
          releaseOfficial = () => {
            order.push("official");
            resolve({ officialUsage: { collectedAt: 9 }, elapsedMs: 1200, reused: false });
          };
        }),
    );

    const committed: string[] = [];
    const promise = runProgressiveRefresh(
      {
        commit: (next) => committed.push((next as unknown as { marker?: string }).marker ?? "official"),
        refreshCredits: async () => {
          order.push("credits-start");
          await new Promise<void>((resolve) => {
            releaseCredits = () => {
              order.push("credits-done");
              resolve();
            };
          });
        },
        loadAccountIds: async () => ["a1", "a2"],
      },
      {
        onLocal: () => order.push("phase-local"),
        onCredits: () => order.push("phase-credits"),
        onOfficial: () => order.push("phase-official"),
        onPhase: () => undefined,
      },
    );

    // 让本地读取先完成。
    await Promise.resolve();
    await Promise.resolve();
    expect(order[0]).toBe("local");
    expect(order).toContain("credits-start");
    expect(order).not.toContain("phase-credits");

    // 官方先完成：应该先出现 official，再等积分。
    (releaseOfficial as unknown as () => void)();
    await Promise.resolve();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(order).toContain("official");

    (releaseCredits as unknown as () => void)();
    await promise;
    expect(order).toContain("phase-credits");
    expect(order).toContain("phase-official");
    // 三次提交：本地 → 积分 → 官方覆盖。
    expect(committed.length).toBeGreaterThanOrEqual(3);
  });

  it("无账号时跳过积分任务但仍标记完成", async () => {
    getCreditStatistics.mockResolvedValue(stats("local"));
    refreshOfficialUsage.mockResolvedValue({
      officialUsage: { collectedAt: 1 },
      elapsedMs: 10,
      reused: false,
    });
    const phases: string[] = [];
    await runProgressiveRefresh(
      {
        commit: () => undefined,
        refreshCredits: async () => {
          throw new Error("不应被调用");
        },
        loadAccountIds: async () => [],
      },
      {
        onLocal: () => phases.push("local"),
        onCredits: () => phases.push("credits"),
        onOfficial: () => phases.push("official"),
        onPhase: (patch) => {
          if (patch.credits) phases.push("phase-credits");
        },
      },
    );
    expect(phases).toContain("phase-credits");
  });
});

import { describe, expect, it } from "vitest";

/**
 * 积分页的**源码级回归锁**。
 *
 * 为什么不是渲染测试：本仓 vitest 是 `environment: "node"`、`include: ["src/**\*.test.ts"]`，
 * 没有 jsdom / happy-dom / @testing-library，`CreditStatsPage`（含 store、effect、Radix
 * 弹窗）无法在这里挂载。真正能跑的断言只有「纯逻辑」与「源码形状」，这一组锁的是两条
 * **已经发生过**的回归，字符串级断言足够稳定（要么在、要么不在）。
 *
 * 两条回归：
 * 1. **陈旧 `creditMap`**：曾经在每次加载时从全局 store 的 `creditMap` 重算失败，
 *    而 `loadCredits` 只补未缓存的 id——已存在的失败条目不会被重查，于是「本轮根本没发起
 *    查询」也会把上一次的失败文案与按钮显示出来。修法：不读全局 store，只吃本轮 stats
 *    返回的 `officialUsage.errors[]`。
 * 2. **后台补采覆盖前台错误**：成功路径曾经无 `if (!background)` 守卫地写错误条，
 *    后台 tick 会用积分失败文案盖掉前台已有的别的错误。修法：成功路径不写错误条。
 */
/**
 * 读页面源码：用 Vite 的 `?raw` 导入（本仓 `src/vite-env.d.ts` 已引 `vite/client` 类型），
 * 避免依赖 `node:fs` 的 @types（本仓没装 `@types/node`，直接 `import "node:fs"` 会挂 typecheck）。
 */
const SOURCE = (
  import.meta.glob("./index.tsx", { query: "?raw", import: "default", eager: true }) as Record<
    string,
    string
  >
)["./index.tsx"];

describe("积分页源码级约束", () => {
  it("不读全局 store 的 creditMap 来重算失败（陈旧状态回归）", () => {
    expect(SOURCE).not.toContain("getState().creditMap");
    // 只允许把 creditMap 当数据传给明细组件，不能当失败来源。
    expect(SOURCE).not.toContain("findCreditFailure(useAccountsStore");
  });

  it("失败动作只从本轮 stats 的 errors[] 派生", () => {
    expect(SOURCE).toContain("findCreditFailure(failureSource?.errors)");
  });

  it("成功路径不写错误条（后台补采不得覆盖前台错误）", () => {
    // 取「拿到本轮统计」到「进入 catch」之间的代码：这段是成功路径。
    const start = SOURCE.indexOf("await loadCreditStatistics(refresh)");
    const end = SOURCE.indexOf("} catch (cause) {", start);
    expect(start).toBeGreaterThan(-1);
    expect(end).toBeGreaterThan(start);
    const successPath = SOURCE.slice(start, end);
    expect(successPath).not.toContain("setError(");
    expect(successPath).not.toContain("setLoadFailure(");
  });

  it("catch 分支写错误条时带 !background 守卫（与既有约定一致）", () => {
    const guarded = "if (!background) {";
    const catchAt = SOURCE.indexOf("} catch (cause) {", SOURCE.indexOf("await loadCreditStatistics"));
    const block = SOURCE.slice(catchAt, catchAt + 400);
    expect(block).toContain(guarded);
    expect(block).toContain("setError(api.asError(cause));");
  });
});

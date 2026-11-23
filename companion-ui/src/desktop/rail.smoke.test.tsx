/**
 * `desktop/rail.tsx` 入口模块的冒烟测试（GitHub issue #5 的回归防线）。
 *
 * 这道测试存在的唯一理由：**真实求值入口模块，并盯住 unhandled rejection**。
 *
 * issue #5 的故障链路是「静默」的：模块顶层 `void boot(container)` 写在
 * `let stopAttention` 之前，`boot()` 虽是 async，但在第一个 `await` 前会同步执行
 * `productionOptions()` → 写 `stopAttention` → 撞上 TDZ 抛 `ReferenceError`；
 * 异常发生在 async 函数体里，于是 `void boot(...)` 把它吞成 unhandled promise
 * rejection。脚本其余部分照常跑完，`<main id="desktop-rail">` 永远没有子节点，
 * 悬浮窗又是 `transparent(true)`，用户看到的是「完全没有悬浮栏」，控制台只有一条
 * 容易漏看的 "(in promise)"。
 *
 * 所以这里**刻意不做任何 mock**：不替换 `boot`、不替换 `productionOptions`、
 * 不替换 transport。任何一处被 mock 掉，TDZ 的触发条件就消失，测试会退化成
 * 「永远绿的壳」——那还不如没有。
 *
 * 与 ESLint 那条 `@typescript-eslint/no-use-before-define` 的分工：
 * - ESLint 管**写法模式**（顶层副作用写到声明前面就报错），快、但只覆盖已知形态；
 * - 本测试管**行为结果**（容器真的被 React 挂上了、且没有静默 rejection），
 *   慢一点，但它拦的是「症状」而不是「某一种写法」。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

/** 入口模块查找的容器 id，与 `desktop.html` 的 `<main>` 保持一致。 */
const RAIL_ELEMENT_ID = 'desktop-rail';

/**
 * Node 的 `unhandledRejection` 钩子的最小结构类型。
 *
 * companion-ui 的 tsconfig 只带 `vite/client` 类型（没有 `@types/node`），为了一个
 * 测试引入整套 Node 类型不划算；也不能用 `any` / `@ts-expect-error` 糊过去
 * （见 AGENTS.md 第 6 节）。这里用**只声明实际用到的那两个方法**的结构类型替代。
 */
interface RejectionHook {
  on(event: 'unhandledRejection', listener: (reason: unknown) => void): void;
  off(event: 'unhandledRejection', listener: (reason: unknown) => void): void;
}

/**
 * 取 Node 的 `process`。happy-dom 只替换 `document` / `window` 等浏览器对象，
 * `process` 仍是 Node 的；取不到就说明运行环境不对，直接失败而不是悄悄跳过断言。
 */
function rejectionHook(): RejectionHook {
  const candidate = (globalThis as {process?: unknown}).process;
  if (!candidate) throw new Error('rail 冒烟测试需要 Node 的 process 事件：请在 vitest 下运行');
  return candidate as RejectionHook;
}

/** 放过一个宏任务：让挂载提交落地，也让潜在 rejection 从 Node 里冒出来。 */
function nextMacrotask(): Promise<void> {
  return new Promise<void>(resolve => { setTimeout(resolve, 0); });
}

/** 被真正构造过的流地址；用来证明走的是生产装配路径，而不是 demo 分支。 */
const openedStreams: string[] = [];

/**
 * happy-dom 21 没有实现 `EventSource`（Node 22 也没有），而生产 rail 在**非 Tauri**
 * 环境下的默认 transport 就是 `new EventSource('/events')`
 * （见 `src/monitor/transport.ts`）。缺了它，`controller.start()` 会抛
 * `ReferenceError: EventSource is not defined`，同样被 async 吞成 rejection——
 * 那是**测试环境**的缺失，不是被测代码的问题，所以只补这一个 API。
 *
 * 刻意保持最小：只实现 transport 真正用到的三个成员（`onmessage` / `onerror` / `close`），
 * 不派发任何事件。本测试断言的是「入口真的挂上了 + 没有静默 rejection」，
 * 数据流正确性不属于这道冒烟测试的职责。
 */
class StubEventSource {
  /** transport 赋值的两个回调；本 stub 不触发它们。 */
  onmessage: ((event: {data: string}) => void) | null = null;
  onerror: ((event: unknown) => void) | null = null;
  readonly url: string;

  constructor(url: string) {
    this.url = url;
    openedStreams.push(url);
  }

  close(): void {}
}

/** 装上 stub，返回把 `globalThis.EventSource` 还原成原值的函数（避免污染后续用例）。 */
function installEventSourceStub(): () => void {
  const scope = globalThis as {EventSource?: unknown};
  const previous = scope.EventSource;
  scope.EventSource = StubEventSource;
  return () => { scope.EventSource = previous; };
}

describe('desktop/rail.tsx 入口模块', () => {
  /** 本次用例捕获到的 unhandled rejection 原因。 */
  let rejections: unknown[] = [];
  /** 具名函数，保证 `off` 能摘掉同一个监听器（匿名函数会泄漏到下一个用例）。 */
  let onRejection: (reason: unknown) => void = () => {};
  /** `EventSource` stub 的复原函数。 */
  let restoreEventSource: () => void = () => {};

  beforeEach(() => {
    rejections = [];
    openedStreams.length = 0;
    onRejection = (reason: unknown) => { rejections.push(reason); };
    rejectionHook().on('unhandledRejection', onRejection);
    restoreEventSource = installEventSourceStub();
    // 复刻 desktop.html 的 DOM：入口模块自己不会建容器，容器在 HTML 里。
    // 必须在动态 import 之前就位，否则 `getElementById` 返回 null，入口静默不挂载。
    document.body.innerHTML = `<main id="${RAIL_ELEMENT_ID}" aria-label="WorkBuddy Shift 会话栏"></main>`;
  });

  afterEach(() => {
    // 必须摘掉：监听器挂到进程上会污染同进程的其它测试文件（vitest 默认多文件共享 worker）。
    rejectionHook().off('unhandledRejection', onRejection);
    restoreEventSource();
    document.body.innerHTML = '';
    // 入口模块顶层有副作用，单个测试文件内不复用模块实例，避免第二次 import 被缓存命中而假绿。
    vi.resetModules();
  });

  it('真实求值入口后 #desktop-rail 挂上了 React 子节点，且没有 unhandled rejection', async () => {
    // 动态 import 而不是顶层 import：要保证 `document.body` 里的容器已就位，
    // 并且这次 import 就是**真正执行**入口模块（含 `void boot(container)`）。
    await import('./rail');

    // `boot()` 先同步 render，再 await 后续的 demo 分支 import；给一个宏任务。
    await nextMacrotask();

    // 先断言 rejection，再断言 DOM：issue #5 的真实症状是 rejection，
    // 这样失败信息里就是那条 `ReferenceError: Cannot access 'stopAttention' before
    // initialization`，而不是一个需要人肉推断的「childElementCount 为 0」。
    expect(rejections).toEqual([]);

    // React 19 的并发渲染可能把提交推迟到下一个任务，用 waitFor 而不是再赌一次 tick。
    await vi.waitFor(() => {
      const rail = document.getElementById(RAIL_ELEMENT_ID);
      expect(rail).not.toBeNull();
      // 悬浮窗是 `transparent(true)` 的独立窗口：容器没有子节点 = 用户看到空气。
      expect(rail?.childElementCount ?? 0).toBeGreaterThan(0);
    });

    // 反「空壳化」：只有真的走 `productionOptions()`（TDZ 的现场）才会构造出这条流。
    // 如果哪天有人把入口改成 demo 分支、或把这些装配 mock 掉，这一条会先失败。
    expect(openedStreams).toEqual(['/events']);
  });
});

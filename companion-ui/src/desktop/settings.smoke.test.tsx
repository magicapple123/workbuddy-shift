/**
 * `desktop/settings.tsx` 入口模块的冒烟测试（与 `rail.smoke.test.tsx` 同一套思路）。
 *
 * 为什么两个入口都要测：它们都是「顶层直接挂载 React」的独立窗口，故障形态也一致——
 * 模块求值时抛出的异常会变成 unhandled rejection，页面静默白屏，而窗口是
 * `transparent(true)`，用户看到的是「设置窗没打开」。rail 那次是 TDZ（issue #5），
 * settings 现在虽然是同步挂载、撞不出 TDZ，但同样可能在求值期踩到缺 API / 缺环境
 * （例如非 Tauri 宿主下的 transport、localStorage 受限、Radix 依赖的 DOM 能力缺失）。
 *
 * 与 ESLint 那条 `no-use-before-define` 的分工：ESLint 管「写法模式」，本测试管
 * 「真实求值后有没有挂上、有没有被吞掉的 rejection」。任何一处 mock 掉挂载路径都会
 * 让这条测试失去意义，所以这里不 mock `createRoot` / `SettingsForm`。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

/** 入口模块查找的容器 id，与 `desktop-settings.html` 的 `<div>` 保持一致。 */
const ROOT_ELEMENT_ID = 'root';

/** Node `unhandledRejection` 钩子的最小结构类型（不引 @types/node，理由同 rail 测试）。 */
interface RejectionHook {
  on(event: 'unhandledRejection', listener: (reason: unknown) => void): void;
  off(event: 'unhandledRejection', listener: (reason: unknown) => void): void;
}

function rejectionHook(): RejectionHook {
  const candidate = (globalThis as {process?: unknown}).process;
  if (!candidate) throw new Error('settings 冒烟测试需要 Node 的 process 事件：请在 vitest 下运行');
  return candidate as RejectionHook;
}

/** 放过一个宏任务：让 React 提交落地，也让潜在 rejection 从 Node 里冒出来。 */
function nextMacrotask(): Promise<void> {
  return new Promise<void>(resolve => { setTimeout(resolve, 0); });
}

/**
 * Radix 的 `Select` / `Switch` 在挂载期会摸几个 happy-dom 没有实现的能力：
 * `ResizeObserver`、`hasPointerCapture` / `releasePointerCapture` / `scrollIntoView`。
 * 缺失时它们会抛错并被 async 吞掉，看上去就像「设置页白屏」——那是**测试环境**的缺口，
 * 不是被测代码的问题，所以只补这几个最小实现（不派发任何事件、不做布局计算）。
 */
function installDomGaps(): () => void {
  const scope = globalThis as {
    ResizeObserver?: unknown;
    PointerEvent?: unknown;
  };
  const previousResize = scope.ResizeObserver;
  const previousPointer = scope.PointerEvent;

  if (!scope.ResizeObserver) {
    scope.ResizeObserver = class {
      observe(): void {}
      unobserve(): void {}
      disconnect(): void {}
    };
  }
  if (!scope.PointerEvent) {
    scope.PointerEvent = class extends Event {
      constructor(type: string, init?: EventInit) { super(type, init); }
    };
  }

  const proto = globalThis.Element?.prototype as
    | (Element & {hasPointerCapture?: unknown; releasePointerCapture?: unknown; scrollIntoView?: unknown})
    | undefined;
  const patched: Array<[string, unknown]> = [];
  if (proto) {
    for (const name of ['hasPointerCapture', 'releasePointerCapture', 'scrollIntoView'] as const) {
      if (typeof proto[name] !== 'function') {
        patched.push([name, proto[name]]);
        Object.defineProperty(proto, name, {value: () => false, configurable: true, writable: true});
      }
    }
  }

  return () => {
    scope.ResizeObserver = previousResize;
    scope.PointerEvent = previousPointer;
    for (const [name, previous] of patched) {
      if (previous === undefined) delete (proto as unknown as Record<string, unknown>)[name];
      else Object.defineProperty(proto as object, name, {value: previous, configurable: true, writable: true});
    }
  };
}

describe('desktop/settings.tsx 入口模块', () => {
  let rejections: unknown[] = [];
  let onRejection: (reason: unknown) => void = () => {};
  let restoreDom: () => void = () => {};

  beforeEach(() => {
    rejections = [];
    onRejection = (reason: unknown) => { rejections.push(reason); };
    rejectionHook().on('unhandledRejection', onRejection);
    restoreDom = installDomGaps();
    // 设置页挂载后会去读偏好与监听源；脱离 Tauri 宿主时它走的是 WebUI 的
    // `http://127.0.0.1:3000`。真实环境中没有那个服务，测试里也不该依赖它：
    // 让它立刻失败，既避免 16s 的连接超时噪音，也保证这条冒烟测试只断言
    // 「入口能求值、能挂载、没有静默 rejection」，失败分支本身由 SettingsForm 处理。
    vi.stubGlobal('fetch', () => Promise.reject(new Error('settings 冒烟测试不连真实 WebUI')));
    // 复刻 desktop-settings.html 的 DOM：入口模块自己不会建容器。
    document.body.innerHTML = `<div id="${ROOT_ELEMENT_ID}"></div>`;
  });

  afterEach(() => {
    rejectionHook().off('unhandledRejection', onRejection);
    restoreDom();
    vi.unstubAllGlobals();
    document.body.innerHTML = '';
    // 入口模块顶层有副作用，单文件内不复用模块实例，避免第二次 import 命中缓存而假绿。
    vi.resetModules();
  });

  it('真实求值入口后 #root 挂上了设置表单，且没有 unhandled rejection', async () => {
    // 动态 import 而不是顶层 import：容器必须先就位，且这次 import 就是真正执行入口模块
    // （含 `installStandaloneWindowChrome()` 与 `createRoot(container).render(...)`）。
    await import('./settings');
    await nextMacrotask();

    // 先断言症状（被吞掉的异常），失败信息才是原因本身，而不是一个「children 为 0」。
    expect(rejections).toEqual([]);

    await vi.waitFor(() => {
      const root = document.getElementById(ROOT_ELEMENT_ID);
      expect(root).not.toBeNull();
      expect(root?.childElementCount ?? 0).toBeGreaterThan(0);
    });

    // 反「空壳化」：只有真的渲染了 SettingsForm，才会有 `.rail-settings` 与标题文本。
    // 若有人把入口改成只挂一个空 div、或用 `vi.mock` 打掉组件，这一条会先红。
    const page = document.querySelector('.rail-settings');
    expect(page).not.toBeNull();
    expect(page?.querySelector('h1')?.textContent ?? '').toContain('悬浮窗设置');
    // 两个头像风格按钮取自 `styles`，证明表单主体（而不只是外壳）渲染完成。
    expect(page?.querySelectorAll('[data-style]').length ?? 0).toBeGreaterThanOrEqual(2);
  });
});

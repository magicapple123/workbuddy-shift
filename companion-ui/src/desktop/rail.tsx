// 主题最先执行（见 settings.tsx 的同样原因）：theme.ts 在求值时就切好
// <html> 的 dark class，rail.css 随后才注入，避免先闪一帧浅色。
import { watchTheme } from './theme';
import { createRoot } from 'react-dom/client';
import { RailApp } from './components/Rail';
import { createRailController, type RailControllerOptions } from './rail-controller';
import { createSSETransport } from '../monitor/transport.js';
import { withPermissionAttention } from '../monitor/permission-attention.js';
import { disableNativeContentDrag } from './host.js';
import './rail.css';

// A rail is a window, not a document: nothing in it may start a native drag.
disableNativeContentDrag();

/**
 * 主应用改主题时，已打开的会话栏要立即跟着变（同一份 localStorage）。
 * 悬浮窗是独立窗口，`storage` 事件不一定跨窗口派发，theme.ts 内还补了
 * focus / visibilitychange 重读，这里只负责在窗口销毁时退订。
 */
const stopThemeWatch = watchTheme();

/** Set only for the production wiring; the demo build has no real stream. */
let stopAttention: (() => void) | null = null;

/**
 * Production wiring: the real snapshot stream, plus the permission-gate
 * highlight. The wrapper only observes the same snapshots the rail already
 * renders — it never sends a decision back to a client (see
 * `monitor/permission-attention.ts`).
 */
function productionOptions(target: HTMLElement): RailControllerOptions {
  const attention = withPermissionAttention(createSSETransport().subscribe, {container: target});
  stopAttention = attention.dispose;
  return {subscribe: attention.subscribe};
}

/**
 * The demo build runs this same rail against the stage page that embeds it
 * instead of against a host. The choice is a build-time constant, so a query
 * parameter can never turn the production rail into the demo one; in the
 * production build the branch is dead code and the demo module is never
 * emitted.
 */
async function boot(target: HTMLElement) {
  const demo = import.meta.env.VITE_DEMO_MODE === '1';
  const controller = demo
    ? createRailController((await import('../demo/rail-bridge.js')).createDemoRailOptions())
    : createRailController(productionOptions(target));
  // `#desktop-rail` is the React container, so React never writes its
  // attributes; the controller owns the classes and datasets that describe the
  // window as a whole (inactive, motion-paused, welcome phase).
  controller.attach.container(target);
  createRoot(target).render(<RailApp controller={controller} container={target} />);
  window.addEventListener('pagehide', () => {
    controller.dispose();
    stopAttention?.();
    stopThemeWatch();
  }, {once: true});
  controller.start();
}

/**
 * 挂载放在模块**最末尾**，不能提前。
 *
 * `boot()` 虽然是 async，但它在第一个 `await` 之前会同步执行
 * `productionOptions(target)`，而后者要写 `stopAttention`。如果这个调用写在
 * `let stopAttention` 之前，模块顶层就会撞上 TDZ：抛错发生在 async 函数里，
 * 变成 unhandled promise rejection，页面静默失败——`#desktop-rail` 永远没有子节点，
 * 窗口又是 `transparent(true)`，用户看到的是「完全没有悬浮栏」（issue #5，0.2.4–0.3.0；
 * 0.2.0–0.2.3 的 `boot()` 是 `createRailController()` 空参调用，碰不到 `stopAttention`）。
 * 放在末尾可以保证所有顶层绑定都已初始化；ESLint 对这两个入口文件开了
 * `@typescript-eslint/no-use-before-define` 兜底（见 eslint.config.js）。
 */
const container = document.getElementById('desktop-rail');
if (container) void boot(container);

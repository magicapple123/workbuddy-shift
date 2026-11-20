import { invoke } from "@tauri-apps/api/core";
import React from "react";
import ReactDOM from "react-dom/client";
import "@fontsource-variable/bricolage-grotesque";
import App from "./App";
import "./index.css";
import { AppErrorBoundary } from "./components/app-error-boundary";
import { installGlobalErrorHandlers } from "./lib/error-report";
import { installNotificationArchive } from "./lib/notify";
import { applyTheme, getThemePreference, watchSystemTheme } from "./lib/theme";
import { parseWebviewVersion, readWebviewSupport, shouldBlockRender } from "./lib/webview-support";

/**
 * R5-17：WebKit 能力不足时**不挂载 React**。
 *
 * `blocked` 档位的整页提示由 `index.html` 的内联脚本注入（未分层内联样式，老 WebKit 也认），
 * 而 React 第一次渲染会清空 `#root`，把那条提示顶掉。所以这里只做两件事：
 * 置位 `__WB_MOUNTED__`（让启动兜底不要再等 30 秒并替换页面），然后什么都不做。
 */
const webviewSupport = readWebviewSupport();

if (shouldBlockRender(webviewSupport)) {
  window.__WB_MOUNTED__ = true;
  // 第二道拦截：再弹一个原生对话框（不依赖任何 CSS），用户确认后进程退出。
  // 只在桌面壳里调；WebUI 没有进程可退，内联提示页已经够了。
  if (typeof window !== "undefined" && "__TAURI_INTERNALS__" in window) {
    void invoke("report_unsupported_webview", {
      detail: blockedDetail(),
    }).catch(() => {
      /* 弹不出来也不影响已渲染的内联提示页 */
    });
  }
} else {
  applyTheme(getThemePreference());
  const stopWatchingSystemTheme = watchSystemTheme();
  if (import.meta.hot) import.meta.hot.dispose(stopWatchingSystemTheme);

  // 提示存档必须在首个 toast 之前装好（包装 sonner 的四类提示）。
  installNotificationArchive();

  // 全局错误捕获要在挂载前装好：事件回调与异步链路的异常归它兜底（渲染期另有 ErrorBoundary）。
  installGlobalErrorHandlers();

  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <AppErrorBoundary>
        <App />
      </AppErrorBoundary>
    </React.StrictMode>,
  );

  // 入口 JS 已接管首屏：index.html 的静态兜底脚本据此不再替换占位（否则 10 秒后会把
  // 可用的界面或错误页换成「启动失败」提示）。
  window.__WB_MOUNTED__ = true;

  // 挂载成功后清掉「已重载过几次」的计数：本次是慢启动而非真故障，
  // 下一次冷启动应当重新拿满三次静默重载的机会。
  try {
    window.sessionStorage.removeItem("wb-boot-attempt");
  } catch {
    /* 存储不可用时忽略：兜底脚本那边也会按「最后一次」处理，最多不重载，不会循环 */
  }
}

/**
 * 原生对话框的正文：把实测到的 WebKit 版本与建议写清楚。
 *
 * 单独抽成函数是为了在 `blocked` 分支里可读——那段代码在 React 挂载之前执行，
 * 不能依赖任何组件或 hooks。
 */
function blockedDetail(): string {
  const detected = parseWebviewVersion(
    typeof navigator === "undefined" ? "" : navigator.userAgent,
  );
  const version = detected.safari ?? detected.appleWebKit;
  const described = version ? `检测到 ${version}。` : "";
  return [
    "当前系统版本过低，界面无法正常显示。",
    "",
    `${described}本应用界面需要 Safari / WebKit 16.4 及以上。`,
    "macOS 请升级到 13.3+（或把 Safari 更新到最新）；Windows / Linux 请更新系统 WebView。",
    "",
    "点「确定」退出，升级后重新打开即可。",
  ].join("\n");
}

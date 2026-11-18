import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { useVisibleInterval } from "@/hooks/use-visible-interval";
import * as api from "@/lib/api";
import { useAccountsStore } from "@/stores/accounts";

export const WORKBUDDY_STATUS_REFRESH_INTERVAL_MS = 60 * 1000;

/**
 * Refresh WorkBuddy status only while the main window is visible and focused.
 *
 * 与积分刷新共用 `useVisibleInterval` 的可见性与定时器；这里保留本 hook 专属的三件事：
 * 1. 桌面端等 `isVisible()` 给出权威初值再开始轮询（Tauri 启动事件可能早于监听安装）；
 * 2. focus / blur 也参与活性判定（后台窗口不轮询）；
 * 3. 失活时中止在途请求（AbortController）。
 */
export function useWorkbuddyStatusRefresh() {
  const [active, setActive] = useState(false);
  const abortControllerRef = useRef<AbortController | undefined>(undefined);

  useEffect(() => {
    let disposed = false;
    let documentVisible = document.visibilityState !== "hidden";
    const webui = api.isWebui();
    // 桌面端先保持 inactive，等 isVisible() 给出权威初值；后续变化走事件。
    let mainWindowVisible = webui;
    let windowFocused = document.hasFocus();

    const sync = () => {
      if (disposed) return;
      setActive(documentVisible && mainWindowVisible && windowFocused);
    };
    const onVisibilityChange = () => {
      documentVisible = document.visibilityState !== "hidden";
      sync();
    };
    const onFocus = () => {
      windowFocused = true;
      sync();
    };
    const onBlur = () => {
      windowFocused = false;
      sync();
    };

    document.addEventListener("visibilitychange", onVisibilityChange);
    window.addEventListener("focus", onFocus);
    window.addEventListener("blur", onBlur);
    sync();

    let unlisten: (() => void) | undefined;
    if (!webui) {
      void (async () => {
        try {
          const fn = await listen<boolean>("main-window-visible", (event) => {
            mainWindowVisible = event.payload;
            sync();
          });
          if (disposed) {
            fn();
            return;
          }
          unlisten = fn;

          try {
            mainWindowVisible = await getCurrentWindow().isVisible();
          } catch {
            // 原生查询不可用时沿用 DOM 可见性；仍要求窗口聚焦才会开始轮询。
            mainWindowVisible = documentVisible;
          }
          if (disposed) return;
          sync();
        } catch {
          if (!disposed) {
            mainWindowVisible = documentVisible;
            sync();
          }
        }
      })();
    }

    return () => {
      disposed = true;
      document.removeEventListener("visibilitychange", onVisibilityChange);
      window.removeEventListener("focus", onFocus);
      window.removeEventListener("blur", onBlur);
      unlisten?.();
    };
  }, []);

  // 活性变化即重建/中止请求；声明顺序保证 AbortController 先于定时器的立即回调就绪。
  useEffect(() => {
    if (!active) {
      abortControllerRef.current?.abort();
      abortControllerRef.current = undefined;
      return;
    }
    abortControllerRef.current = new AbortController();
    return () => {
      abortControllerRef.current?.abort();
      abortControllerRef.current = undefined;
    };
  }, [active]);

  function refreshStatus() {
    void useAccountsStore.getState().refreshStatus(abortControllerRef.current?.signal);
  }

  useVisibleInterval(refreshStatus, WORKBUDDY_STATUS_REFRESH_INTERVAL_MS, active);
}

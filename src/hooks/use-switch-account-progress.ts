import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";

import * as api from "@/lib/api";

/**
 * 切换进度：桌面端订阅 Tauri switch-progress 事件，webui 走 600ms HTTP 轮询。
 *
 * 返回 [进度文案, 设置进度]：发起切换时由调用方写入「正在切换账号…」，结束后清空；
 * 后端上报的进度覆盖同一份 state。
 */
export function useSwitchAccountProgress(): [string, (message: string) => void] {
  const [progress, setProgress] = useState("");

  // 监听后端切换进度：桌面端走 Tauri 事件，webui 走 HTTP 轮询
  useEffect(() => {
    if (api.isWebui()) {
      const timer = setInterval(() => {
        void api.switchProgress().then((p) => {
          if (p.progress) setProgress(p.progress);
        });
      }, 600);
      return () => clearInterval(timer);
    }
    let disposed = false;
    let unlisten: (() => void) | undefined;
    listen<{ message: string }>("switch-progress", (e) => {
      setProgress(e.payload.message);
    }).then((fn) => {
      if (disposed) {
        fn();
        return;
      }
      unlisten = fn;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  return [progress, setProgress];
}

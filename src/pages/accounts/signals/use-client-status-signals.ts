import { useEffect, useRef } from "react";

import { useVisibleInterval } from "@/hooks/use-visible-interval";
import * as api from "@/lib/api";
import type { ToolId } from "@/lib/supported-tools";
import type { WbVariant } from "@/lib/types";
import { variantUsesIntlCodebuddyIde } from "@/lib/variant";
import { useAccountsStore } from "@/stores/accounts";

/**
 * CodeBuddy CLI 认证状态的重读间隔。
 *
 * 保活刷新会先批量改写账号库里的 token、再把新 token 同步回 `~/.codebuddy/settings.json`。
 * 这个窗口里状态判定会短暂认为「认证已脱节」。后端在刷新前后各广播一次
 * `codebuddy-cli-updated`，这里再挂一个可见时轮询兜底。
 */
const CLI_STATUS_REFRESH_INTERVAL_MS = 30 * 1000;

interface Options {
  variant: WbVariant;
  /** 账号数量变化时重新探测（切档位后账号集合不同）。 */
  accountsLength: number;
  /** 支持工具开关：关闭的端既不探测也不轮询（与入口隐藏保持一致）。 */
  enabledTools: Record<ToolId, boolean>;
}

/**
 * 各客户端（CLI / IDE / VS Code / JetBrains）状态探测与刷新。
 *
 * 状态本体放在 store 里：账号页每次进入都会重挂载，局部 state 会被重置为 null，
 * 界面先按「未接入」渲染（issue #84）。这里只负责探测、事件订阅与可见时轮询。
 */
export function useClientStatusSignals({ variant, accountsLength, enabledTools }: Options) {
  const clientStatus = useAccountsStore((state) => state.clientStatus);
  const setClientStatus = useAccountsStore((state) => state.setClientStatus);

  async function refreshCodebuddyCliStatus() {
    try {
      setClientStatus({ codebuddyCli: await api.getCodebuddyCliStatus() });
    } catch {
      setClientStatus({ codebuddyCli: null });
    }
  }

  async function refreshCodebuddyCnIdeStatus() {
    try {
      setClientStatus({
        codebuddyCnIde: variantUsesIntlCodebuddyIde(variant)
          ? await api.getCodebuddyIdeStatus()
          : await api.getCodebuddyCnIdeStatus(),
      });
    } catch {
      setClientStatus({ codebuddyCnIde: null });
    }
  }

  async function refreshVscodeExtStatus() {
    try {
      setClientStatus({ vscodeExt: await api.getVscodeExtStatus() });
    } catch {
      setClientStatus({ vscodeExt: null });
    }
  }

  async function refreshJetbrainsStatus() {
    try {
      setClientStatus({ jetbrains: await api.getJetbrainsStatus() });
    } catch {
      setClientStatus({ jetbrains: null });
    }
  }

  useEffect(() => {
    let cancelled = false;
    if (enabledTools.codebuddyCli) void refreshCodebuddyCliStatus();

    /** 只读本地状态文件与进程，不碰钥匙串，因此不必等下面的本机登录探测。 */
    async function refreshClientStatuses() {
      if (cancelled) return;
      if (enabledTools.codebuddyIde) await refreshCodebuddyCnIdeStatus();
      if (cancelled) return;
      if (enabledTools.vscodeExt) await refreshVscodeExtStatus();
      if (cancelled) return;
      if (enabledTools.jetbrains) await refreshJetbrainsStatus();
    }

    void (async () => {
      // 状态刷新与登录探测并行起跑：探测要读钥匙串 / Safe Storage / 注册表 / 进程，
      // 排在状态前面会让「已接入」迟迟不显示（issue #84）。
      const statuses = refreshClientStatuses();
      if (!api.isDemoMode()) {
        if (enabledTools.codebuddyIde) {
          try {
            // 国际版探测 CodeBuddy.app 钥匙串；国内版探测 CodeBuddy CN。不要交叉读。
            if (variantUsesIntlCodebuddyIde(variant)) {
              await api.detectCodebuddyIdeAccount();
            } else {
              await api.detectCodebuddyCnIdeAccount();
            }
          } catch {
            /* 未登录或钥匙串拒绝时静默，下面仍拉安装/运行状态 */
          }
        }
        if (enabledTools.vscodeExt) {
          try {
            await api.detectVscodeExtAccount();
          } catch {
            /* VS Code 未登录或 Safe Storage 不可用时静默 */
          }
        }
        if (enabledTools.jetbrains) {
          try {
            await api.detectJetbrainsAccount();
          } catch {
            /* JetBrains 插件未登录时静默 */
          }
        }
      }
      await statuses;
      // 探测命中账号时后端会把「当前账号」写回本地状态，再读一次让高亮跟上。
      if (!cancelled) await refreshClientStatuses();
    })();
    return () => {
      cancelled = true;
    };
  }, [accountsLength, variant, enabledTools]);

  // 事件与轮询都需要最新引用：直接闭包捕获会一直指向挂载时的旧判断。
  const refreshCliRef = useRef(refreshCodebuddyCliStatus);
  refreshCliRef.current = refreshCodebuddyCliStatus;

  useEffect(() => {
    if (api.isWebui() || !enabledTools.codebuddyCli) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void api.listenDesktopEvent("codebuddy-cli-updated", () => {
      void refreshCliRef.current();
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
  }, [enabledTools.codebuddyCli]);

  useVisibleInterval(
    () => void refreshCliRef.current(),
    CLI_STATUS_REFRESH_INTERVAL_MS,
    enabledTools.codebuddyCli,
  );

  return {
    ...clientStatus,
    refreshCodebuddyCliStatus,
    refreshCodebuddyCnIdeStatus,
    refreshVscodeExtStatus,
    refreshJetbrainsStatus,
  };
}

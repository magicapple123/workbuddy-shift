import { useCallback, useState, useSyncExternalStore } from "react";
import { toast } from "sonner";

import * as api from "@/lib/api";
import { demoModeEnabled } from "@/lib/demo-mode";
import { useUpdateState } from "@/hooks/use-update-state";

import { UpdateAvailableCard } from "./update-available-card";
import { UpdateInstallDialog } from "./update-install-dialog";
import { promptVersion, shouldPrompt } from "./update-prompt";

/**
 * 本次会话已「稍后」的版本（模块级、**不落盘**）。
 *
 * 语义是「这一轮先别烦我」：重启进程即清空，同一版本再次发现会重新提示一次。
 * 放在模块级而不是组件 state，是为了让卡片 / 弹窗 / 路由切换共用同一份记录——
 * 切页或切账号（组件重新挂载）不会把「稍后」忘掉，同一版本不会重复弹。
 */
const dismissedVersions = new Set<string>();

/**
 * 更新弹窗的全局开关。
 *
 * 三个入口（侧边栏小箭头 / 设置页「立即升级」/ 右下角卡片「立即更新」）都开同一个弹窗，
 * 弹窗只在这里渲染一份：否则同一次下载会出现两个阶段的弹窗，关掉一个另一个还在。
 */
let dialogOpen = false;
const dialogListeners = new Set<() => void>();

function publishDialog(next: boolean): void {
  if (dialogOpen === next) return;
  dialogOpen = next;
  for (const listener of dialogListeners) listener();
}

/** 打开全局更新弹窗（任意入口均可调用）。 */
export function openUpdateDialog(): void {
  publishDialog(true);
}

function subscribeDialog(listener: () => void): () => void {
  dialogListeners.add(listener);
  return () => {
    dialogListeners.delete(listener);
  };
}

function getDialogOpen(): boolean {
  return dialogOpen;
}

/**
 * 更新宿主：全局唯一的「发现新版本」卡片 + 更新弹窗。
 *
 * 挂在应用外壳（App 的 Layout）里，而不是某一个页面：启动 15 秒后的后台检查、
 * 设置页的手动检查、托盘菜单发现的新版本，都要能在任意页面弹出同一张卡片。
 * 卡片与弹窗的层级关系由卡片自身的 `z-40` / 弹窗的 `z-50` 保证。
 */
export function UpdateHost() {
  const snapshot = useUpdateState();
  const open = useSyncExternalStore(subscribeDialog, getDialogOpen, getDialogOpen);
  // 忽略集合跟随模块级 Set：宿主重新挂载时也能拿回本次会话已忽略的版本。
  const [dismissed, setDismissed] = useState<ReadonlySet<string>>(
    () => new Set(dismissedVersions),
  );

  const version = promptVersion(snapshot);

  /** 「稍后」：只记在内存里，本次会话不再为这个版本弹卡片。 */
  const dismiss = useCallback(() => {
    const target = promptVersion(snapshot);
    if (!target) return;
    dismissedVersions.add(target);
    setDismissed(new Set(dismissedVersions));
  }, [snapshot]);

  /**
   * 卡片「立即更新」：先开弹窗（立刻给出反馈），再交给状态机下载。
   *
   * 进度与失败文案都由 `update-state` 推送，弹窗复用同一份快照；
   * 这里只兜住命令层拒绝（例如已有下载任务在跑）。
   */
  const install = useCallback(() => {
    openUpdateDialog();
    void api.updateDownload().catch((error: unknown) => {
      toast.error("下载更新失败", { description: api.asError(error) });
    });
  }, []);

  // 演示模式没有真实的启动检查，卡片会常驻遮住演示页内容（按钮又被 DemoAction 禁用，
  // 用户关不掉）。演示页的更新入口仍由设置页的结果区提供。
  const cardVisible = !demoModeEnabled && Boolean(version) && shouldPrompt(snapshot, dismissed);

  return (
    <>
      {cardVisible && version ? (
        <UpdateAvailableCard
          version={version}
          notes={snapshot.notes}
          publishedAt={snapshot.publishedAt}
          onInstall={install}
          onDismiss={dismiss}
        />
      ) : null}
      <UpdateInstallDialog
        open={open}
        onOpenChange={(next) => publishDialog(next)}
      />
    </>
  );
}

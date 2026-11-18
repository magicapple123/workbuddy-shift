import type { ReactNode } from "react";
import { CircleCheck, RefreshCw, TriangleAlert } from "lucide-react";

import type { ClientSwitchClient, ClientSwitchStatus, StatusNotice } from "@/components/client-switch-account-types";

/**
 * 客户端切号弹窗的按客户端文案：状态三态提示、空态、未登录说明与忙碌遮罩。
 *
 * 两个客户端（VS Code 插件 / CodeBuddy IDE）共用同一套弹窗结构，差异只在文字与
 * 「是否有自动关闭并重开开关」；集中在这里便于逐字核对，弹窗本体只负责组装。
 */

/** 标题行状态图标：三态提示收进 tooltip，仅以图标色调区分正常/警告。 */
export function statusNoticeFor(
  client: ClientSwitchClient,
  running: boolean,
  autoRestart: boolean,
): StatusNotice {
  if (client === "vscode") {
    if (running) {
      return autoRestart
        ? {
            icon: <RefreshCw className="size-4" />,
            title: "将自动关闭并重开 VS Code",
            description:
              "将先关闭 VS Code（未保存内容由 VS Code 自身提示/热退出保护），写入凭证后自动重新打开。",
            warning: false,
          }
        : {
            icon: <TriangleAlert className="size-4" />,
            title: "请先完全退出 VS Code",
            description:
              "已关闭「自动关闭并重开」。检测到 VS Code 正在运行，运行中写入会被覆盖且不会生效，请完全退出后重试。",
            warning: true,
          };
    }
    return {
      icon: <CircleCheck className="size-4" />,
      title: "VS Code 未运行，可直接切换",
      description: "写入凭证后打开 VS Code，插件即为目标账号。",
      warning: false,
    };
  }
  return running
    ? {
        icon: <RefreshCw className="size-4" />,
        title: "将自动关闭并重开 CodeBuddy IDE",
        description:
          "将先关闭 CodeBuddy IDE（未保存内容由 IDE 自身提示保护），写入凭证后自动重新打开。",
        warning: false,
      }
    : {
        icon: <CircleCheck className="size-4" />,
        title: "CodeBuddy IDE 未运行，可直接切换",
        description: "写入凭证后自动打开 CodeBuddy IDE，即为目标账号。",
        warning: false,
      };
}

/** 未登录时的追加说明（三态提示共用，渲染为 tooltip 内第二段）。 */
export function notLoggedInHint(client: ClientSwitchClient): ReactNode {
  return client === "vscode"
    ? "未检测到 VS Code CodeBuddy 插件登录态，将按新会话写入；切换后打开 VS Code 即登录为目标账号。"
    : "未检测到 CodeBuddy IDE 登录态，将按新登录写入；切换后打开 IDE 即登录为目标账号。";
}

/** 统一空态文案：区分未装客户端 / 未装插件 / 未找到数据目录 / 未登录 / 无会话。 */
export function emptyStateHint(
  client: ClientSwitchClient,
  status: ClientSwitchStatus | null | undefined,
  loading: boolean,
  sourceUid: string | null,
  dataRoot: string | null | undefined,
  hasCopyable: boolean,
): string {
  const vscode = client === "vscode";
  if (loading) return "正在加载会话…";
  if (status && !status.installed) {
    return vscode ? "未检测到 VS Code，请先安装并登录 CodeBuddy 插件" : "未检测到 CodeBuddy IDE，请先安装并登录";
  }
  if (vscode && status && !status.extensionInstalled) {
    return "未安装 VS Code CodeBuddy 插件，请先在 VS Code 中安装并登录";
  }
  if (dataRoot === null) {
    return vscode
      ? "未找到 VS Code CodeBuddy 插件数据目录，请先打开 VS Code 并登录插件"
      : "未找到 CodeBuddy IDE 数据目录，请先打开 IDE 并登录一次";
  }
  if (!sourceUid) {
    return vscode
      ? "未检测到 VS Code CodeBuddy 插件当前登录账号，请先在 VS Code 中登录"
      : "未检测到 CodeBuddy IDE 当前登录账号，请先在 IDE 中登录";
  }
  if (!hasCopyable) return "当前账号暂无可复制的会话（无含正文的历史）";
  return vscode
    ? "将当前账号勾选的会话以新 id 复制给目标账号（加法，不影响源账号）"
    : "将当前账号勾选的会话复制给目标账号（沿用会话 id，目标已有同 id 时自动改用新 id）";
}

/** 忙碌遮罩的主文案与补充说明：按客户端与本次动作（重开 / 复制 / 同步 / 纯切换）分支。 */
export function busyCopy(
  client: ClientSwitchClient,
  options: { autoClose: boolean; copyCount: number; syncCount: number },
): { title: string; hint: string } {
  const { autoClose, copyCount, syncCount } = options;
  if (client === "vscode") {
    return {
      title: autoClose
        ? "正在关闭 VS Code 并写入凭证…"
        : copyCount > 0
          ? "正在切换并复制会话…"
          : syncCount > 0
            ? "正在切换并同步会话…"
            : "正在切换账号…",
      hint: autoClose
        ? "若 VS Code 弹出保存提示请先处理（最多等待 60 秒）"
        : "正在处理中，请勿关闭窗口",
    };
  }
  return {
    title:
      copyCount > 0
        ? "正在关闭 CodeBuddy IDE 并复制会话…"
        : syncCount > 0
          ? "正在关闭 CodeBuddy IDE 并同步会话…"
          : "正在关闭 CodeBuddy IDE 并写入凭证…",
    hint: "若 IDE 弹出保存提示请先处理（最长等待 60 秒）",
  };
}

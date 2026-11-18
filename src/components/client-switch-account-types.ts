import type { ReactNode } from "react";

/**
 * 客户端切号弹窗（VS Code 插件 / CodeBuddy IDE）共用的类型。
 *
 * 弹窗本体在 `client-switch-account-dialog.tsx`，按客户端参数化的文案/空态在
 * `client-switch-account-copy.tsx`，结果提示在 `client-switch-account-result.ts`。
 */

/** 目标客户端：决定文案、状态三态与 API 通道。 */
export type ClientSwitchClient = "vscode" | "codebuddy-ide";

/** 两个客户端状态里弹窗用到的公共字段（VS Code 额外有 `extensionInstalled`）。 */
export interface ClientSwitchStatus {
  installed?: boolean;
  extensionInstalled?: boolean;
  running?: boolean;
  loggedIn?: boolean;
}

/** 状态图标三态提示（收进 tooltip，仅以图标色调区分正常/警告）。 */
export interface StatusNotice {
  icon: ReactNode;
  title: string;
  description: string;
  warning: boolean;
}

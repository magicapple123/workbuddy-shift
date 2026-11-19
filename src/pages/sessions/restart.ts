import type { SessionGroupClient } from "@/lib/types";

/**
 * 插件型客户端的「运行中 → 关闭并重开宿主」逻辑（纯函数，供页面与单测共用）。
 *
 * 两个插件型客户端**判定口径不同**，因此不能共用一条预检命令：
 * - VS Code：只有「本次写入目标包含当前登录账号」时才需要关闭（扩展只写当前账号的数据）；
 * - JetBrains：只要还有装了插件且在运行的 IDE 就必须关闭——IDEA 与 PyCharm 共用同一份
 *   `Data/<uid>/JetBrains`，只关一个仍会被另一个的内存态覆盖。
 *
 * 抽成纯函数是为了让「按客户端分流」这件事可以被单测钉住：一旦有人在页面里把两者合并回
 * 一条分支，JetBrains 用户就会在「切了没变」和「白关一次 IDE」之间反复。
 */

/** 需要「关闭并重开宿主」确认框的客户端；`null` = 该客户端没有宿主进程（WorkBuddy / CodeBuddy IDE）。 */
export type RestartPrecheckClient = "vscodeExt" | "jetbrains";

/** 该客户端是否需要宿主生命周期预检（含关闭后重开）。 */
export function restartPrecheckClient(client: SessionGroupClient): RestartPrecheckClient | null {
  if (client === "vscodeExt") return "vscodeExt";
  if (client === "jetbrains") return "jetbrains";
  return null;
}

export interface RestartDialogCopy {
  title: string;
  description: string;
  /** 处理中提示（关闭 + 写入 + 重开期间显示）。 */
  busyText: string;
}

/** 运行中确认框的文案；`null` = 该客户端没有这个确认框。 */
export function restartDialogCopy(client: SessionGroupClient): RestartDialogCopy | null {
  if (client === "jetbrains") {
    return {
      title: "同步前需要重启 JetBrains IDE",
      description:
        "检测到 JetBrains IDE（IDEA / PyCharm）正在运行：运行中的写入会被 IDE 覆盖。确认后将先关闭所有装了插件且在运行的 IDEA / PyCharm（未保存内容由 IDE 自身提示保护，最多等待 60 秒），完成后再逐个自动重新打开。",
      busyText: "正在关闭 JetBrains IDE 并处理，请勿关闭本窗口…",
    };
  }
  if (client === "vscodeExt") {
    return {
      title: "同步前需要重启 VS Code",
      description:
        "检测到 VS Code 正在运行：运行中的写入会被编辑器覆盖。确认后将先关闭 VS Code（未保存内容由 VS Code 自身提示保护，最多等待 60 秒），完成后再自动重新打开。",
      busyText: "正在关闭 VS Code 并处理，请勿关闭本窗口…",
    };
  }
  return null;
}

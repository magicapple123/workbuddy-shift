import type { UpdateSnapshot } from "@/lib/types";
import { formatDate } from "@/lib/format";

/**
 * 「发现新版本」卡片的展示判定（纯函数，无 React / DOM 依赖）。
 *
 * 卡片只在 `available` 阶段出现：下载中 / 已就绪要让位给下载弹窗，
 * 检查中与失败态也不该常驻一个操作不了的气泡。
 */
export function shouldPrompt(
  snapshot: UpdateSnapshot | null | undefined,
  dismissed: ReadonlySet<string>,
): boolean {
  if (!snapshot) return false;
  if (snapshot.phase !== "available") return false;
  const version = promptVersion(snapshot);
  if (!version) return false;
  return !dismissed.has(version);
}

/**
 * 版本键：用于「稍后」去重。
 *
 * 去掉 `v` 前缀与首尾空白：同一个版本不论从哪个入口（启动检查 / 手动检查）
 * 发现，都必须命中同一条「本次会话已忽略」记录。
 */
export function promptVersion(snapshot: UpdateSnapshot | null | undefined): string | null {
  const version = snapshot?.latest?.trim().replace(/^[vV]/, "");
  return version ? version : null;
}

/** 发布日期的展示文案；没有合法时间返回 `undefined`（卡片不渲染这一行）。 */
export function publishedLabel(publishedAt: string | null | undefined): string | undefined {
  if (!publishedAt) return undefined;
  const parsed = Date.parse(publishedAt);
  if (Number.isNaN(parsed)) return undefined;
  return `发布于 ${formatDate(parsed)}`;
}

/** 说明的折叠阈值：卡片默认展示三行（与 `line-clamp-3` 一致）。 */
export const NOTES_PREVIEW_LINES = 3;

/**
 * 说明是否需要「展开/收起」。
 *
 * 多行说明只要超过三行，或者单段长文本长度超过按行宽估算的容量，就认为被折叠。
 * 这是「按内容判定」的近似：真实溢出由卡片测量 DOM 兜底，两者任一为真即显示开关，
 * 宁可多给一个开关，也不要让用户看不到被裁掉的说明。
 */
export function notesNeedToggle(notes: string | null | undefined): boolean {
  const text = (notes ?? "").trim();
  if (!text) return false;
  const lines = text.split("\n");
  if (lines.length > NOTES_PREVIEW_LINES) return true;
  return text.length > 120;
}

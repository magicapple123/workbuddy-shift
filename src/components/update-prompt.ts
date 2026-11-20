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

/** 卡片摘要的字符上限；与 `scripts/gen-update-json.sh` 的生成侧上限保持一致。 */
export const NOTES_MAX_CHARS = 600;

/** release notes 里的「卡片摘要」区块标记（生成侧优先取这一段）。 */
const CARD_BLOCK_BEGIN = "<!-- card:begin -->";
const CARD_BLOCK_END = "<!-- card:end -->";

/** 行首 Markdown 标记：标题 / 引用 / 无序列表 / 有序列表。 */
const HEADING_RE = /^\s{0,3}#{1,6}\s*/;
const QUOTE_RE = /^\s{0,3}>\s*/;
const BULLET_RE = /^\s*[-*+]\s+/;
const ORDERED_RE = /^\s*\d+[.)]\s+/;
/** 行内标记：HTML 注释、`![alt](src)` / `[text](url)`。 */
const COMMENT_RE = /<!--[\s\S]*?-->/g;
const LINK_RE = /!?\[([^\]]*)\]\([^)]*\)/g;
/** `发布日期：2026-11-19` 这类只属于 Release 页的元信息行。 */
const DATE_RE = /^发布日期\s*[:：]/;

/** 取 `<!-- card:begin -->` … `<!-- card:end -->` 区块；没有标记返回 `null`。 */
function extractCardBlock(text: string): string | null {
  const start = text.indexOf(CARD_BLOCK_BEGIN);
  if (start < 0) return null;
  const bodyStart = start + CARD_BLOCK_BEGIN.length;
  const end = text.indexOf(CARD_BLOCK_END, bodyStart);
  if (end < 0) return null;
  return text.slice(bodyStart, end);
}

/** 表格分隔行：`| --- | --- |`（也接受 `--- | ---`）。 */
function isTableSeparator(line: string): boolean {
  const stripped = line.trim();
  if (!stripped.includes("|") || !stripped.includes("---")) return false;
  return /^[-:|\s]+$/.test(stripped);
}

function isTableLine(line: string): boolean {
  return isTableSeparator(line) || (line.match(/\|/g)?.length ?? 0) >= 2;
}

/** 整段丢弃表格：表头 + 分隔行 + 其后的数据行（遇空行结束）。 */
function dropTables(lines: string[]): string[] {
  const kept: string[] = [];
  let index = 0;
  while (index < lines.length) {
    const line = lines[index];
    const following = lines[index + 1] ?? "";
    if (line.includes("|") && isTableSeparator(following)) {
      index += 2;
      while (index < lines.length && lines[index].includes("|") && lines[index].trim()) {
        index += 1;
      }
      continue;
    }
    if (isTableLine(line)) {
      index += 1;
      continue;
    }
    kept.push(line);
    index += 1;
  }
  return kept;
}

/** 单行清洗：去 HTML 注释 / 链接 / 行首标记 / 行内强调标记。 */
function cleanLine(line: string): string {
  return line
    .replace(COMMENT_RE, "")
    .replace(LINK_RE, "$1")
    .replace(HEADING_RE, "")
    .replace(QUOTE_RE, "")
    .replace(BULLET_RE, "")
    .replace(ORDERED_RE, "")
    .replace(/\*\*/g, "")
    .replace(/__/g, "")
    .replace(/`/g, "")
    .trim();
}

/** 按行边界截断到 `NOTES_MAX_CHARS`；被截掉内容时补一个省略号。 */
function truncateNotes(lines: string[]): string {
  const kept: string[] = [];
  let used = 0;
  for (const line of lines) {
    const extra = line.length + (kept.length > 0 ? 1 : 0);
    if (used + extra > NOTES_MAX_CHARS) {
      // 第一行本身就超限（没有换行的长段落）：只能按字符硬截断。
      if (kept.length === 0) return `${line.slice(0, NOTES_MAX_CHARS - 1)}…`;
      const text = kept.join("\n");
      if (text.length + 1 <= NOTES_MAX_CHARS) return `${text}…`;
      return `${text.slice(0, NOTES_MAX_CHARS - 1)}…`;
    }
    kept.push(line);
    used += extra;
  }
  return kept.join("\n");
}

/**
 * 把清单里的 `notes` 兜底成「卡片摘要（纯文本）」。
 *
 * 为什么前端还要做一遍：生成侧（`scripts/gen-update-json.sh`）已经开始输出摘要，但
 * **已经发布的清单不会回改**——0.2.2 的 `latest*.json` 里躺着整份 Release Markdown，
 * 老客户端会一直收得到。这一层只做「不露 Markdown 标记」的兜底：
 *
 * 1. 优先取 `<!-- card:begin -->` … `<!-- card:end -->` 区块，其次用整份正文；
 * 2. 表格整段丢弃（不做表格→列表的转换：360px 宽度里排不出表）；
 * 3. 标题行整行丢弃（卡片自带「发现新版本 vX」标题，`## 新增` 只会剩下孤行）；
 * 4. 去行首 `>` / 列表标记，去 `**` `__` 反引号，`[文字](链接)` → 文字；
 * 5. 连续空行压成一个，首尾 `trim`，超长按行边界截断到 [`NOTES_MAX_CHARS`]。
 *
 * 不做 Markdown 渲染：本轮明确否决在 360px 卡片里排表格 / 标题。
 */
export function cleanNotes(notes: string | null | undefined): string {
  const raw = (notes ?? "").replace(/\r\n?/g, "\n");
  if (!raw.trim()) return "";
  const body = extractCardBlock(raw) ?? raw;
  const lines: string[] = [];
  let pendingBlank = false;
  for (const line of dropTables(body.split("\n"))) {
    if (HEADING_RE.test(line)) continue;
    const value = cleanLine(line);
    if (!value) {
      pendingBlank = lines.length > 0;
      continue;
    }
    if (DATE_RE.test(value)) continue;
    if (pendingBlank) {
      lines.push("");
      pendingBlank = false;
    }
    lines.push(value);
  }
  while (lines.length > 0 && !lines[0]) lines.shift();
  while (lines.length > 0 && !lines[lines.length - 1]) lines.pop();
  return truncateNotes(lines);
}

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

/**
 * CSV 构造：转义、CRLF、UTF-8 BOM、公式注入防护。
 *
 * - 单元格含 `,` `"` `\r` `\n` 时加引号，内部引号双写；
 * - 文本以 `= + - @ TAB CR` 开头时前置 `'`（Excel / WPS 会当公式执行）；
 *   数字不加前缀，避免把负数变成文本；
 * - 整个文件以 U+FEFF 开头，Excel 打开中文不乱码；
 * - 区块之间用一个空行分隔，便于人读。
 */

export const CSV_BOM = "\uFEFF";

/** 需要防注入的起始字符。 */
const FORMULA_PREFIXES = ["=", "+", "-", "@", "\t", "\r"];

export function escapeCsvCell(value: unknown): string {
  if (value === null || value === undefined) return "";
  if (typeof value === "number") {
    return Number.isFinite(value) ? String(value) : "";
  }
  let text = String(value);
  if (FORMULA_PREFIXES.some((prefix) => text.startsWith(prefix))) {
    text = `'${text}`;
  }
  if (/[",\r\n]/.test(text)) {
    return `"${text.replace(/"/g, '""')}"`;
  }
  return text;
}

export interface CsvSection {
  title: string;
  headers: string[];
  rows: unknown[][];
  /** 该区块的说明行（例如「仅 codebuddy-cli 支持请求明细」）。 */
  note?: string;
}

/** 把若干区块拼成一个 CSV 文本（含 BOM 与 CRLF）。 */
export function buildCsv(sections: CsvSection[]): string {
  const lines: string[] = [];
  sections.forEach((section, index) => {
    if (index > 0) lines.push("");
    lines.push(escapeCsvCell(section.title));
    if (section.note) lines.push(escapeCsvCell(section.note));
    lines.push(section.headers.map(escapeCsvCell).join(","));
    for (const row of section.rows) {
      lines.push(row.map(escapeCsvCell).join(","));
    }
  });
  return `${CSV_BOM}${lines.join("\r\n")}\r\n`;
}

/**
 * 文件名片段清洗：去掉 Windows 非法字符、路径分隔符与空白。
 *
 * 账号显示名可能带 `@`、`/`（例如邮箱、`国内版/国际版`），直接拼进文件名会让
 * 保存对话框失败或写到别的目录。
 */
export function sanitizeFileScope(scope: string): string {
  const cleaned = scope
    .replace(/[\\/:*?"<>|\s]+/g, "-")
    .replace(/-+/g, "-")
    .replace(/^-|-$/g, "")
    .slice(0, 40);
  return cleaned.length > 0 ? cleaned : "all";
}

/** 文件名：`token-stats-<source>-<yyyyMMdd-HHmm>.csv`。 */
export function csvFileName(prefix: string, scope: string, now = new Date()): string {
  const pad = (value: number) => String(value).padStart(2, "0");
  const stamp = `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}-${pad(
    now.getHours(),
  )}${pad(now.getMinutes())}`;
  return `${prefix}-${sanitizeFileScope(scope)}-${stamp}.csv`;
}

/**
 * 最小 XLSX（OOXML SpreadsheetML）生成器：零依赖，浏览器/Node 都能跑。
 *
 * 为什么自己写：项目不引入第三方库（`package.json` 是硬约束，新增依赖需要单独评审）。
 * 这里只需要「多 sheet + 表头样式 + 冻结首行 + 列宽 + 数字类型」这几件事，OOXML 的
 * 子集足够小，用 ZIP(store) + 几个 XML 文件就能产出 Excel / WPS / LibreOffice 都能打开
 * 的工作簿。
 *
 * 产物结构：
 * ```
 * [Content_Types].xml
 * _rels/.rels
 * xl/workbook.xml
 * xl/_rels/workbook.xml.rels
 * xl/styles.xml
 * xl/worksheets/sheet1.xml …
 * ```
 *
 * 关键约定：
 * - 数字写 `<v>`（数值类型，可求和），文本写 `inlineStr`；不生成 sharedStrings，
 *   避免维护索引表；
 * - 首行冻结 + 表头加粗浅底；文本列左对齐、数字列右对齐（样式 id 见 STYLE）；
 * - ZIP 用 store（method 0）：不压缩换来完全同步、零依赖的实现；导出体积在几十 KB
 *   量级，压缩收益可忽略。
 */

/** 单元格值：`null`/`undefined`/空串写成空单元格。 */
export type XlsxCell = string | number | null | undefined;

export interface XlsxSheet {
  /** 工作表名：≤31 字符且不含 `[]:*?/\`（会自动清掉非法字符）。 */
  name: string;
  header: string[];
  rows: XlsxCell[][];
  /** 每列宽度（字符数）；缺省按数字列 14 / 文本列按表头长度估算。 */
  columnWidths?: number[];
}

/** 样式 id 与 styles.xml 的 cellXfs 顺序一一对应。 */
export const STYLE = { default: 0, header: 1, number: 2, text: 3 } as const;

const XML_ESCAPES: Record<string, string> = {
  "&": "&amp;",
  "<": "&lt;",
  ">": "&gt;",
  '"': "&quot;",
  "'": "&apos;",
};

function escapeXml(value: string): string {
  return value.replace(/[&<>"']/g, (char) => XML_ESCAPES[char] ?? char);
}

/** Excel 工作表名：去掉非法字符，截到 31 字符，空名回退 `Sheet`。 */
export function sheetName(name: string): string {
  const cleaned = name.replace(/[[\]:*?/\\]/g, "-").trim().slice(0, 31);
  return cleaned.length > 0 ? cleaned : "Sheet";
}

/** 列号 → 列字母（0 → A，26 → AA）。 */
export function columnName(index: number): string {
  let value = index + 1;
  let letters = "";
  while (value > 0) {
    const remainder = (value - 1) % 26;
    letters = String.fromCharCode(65 + remainder) + letters;
    value = Math.floor((value - 1) / 26);
  }
  return letters;
}

function cellXml(reference: string, value: XlsxCell, style: number): string {
  if (value === null || value === undefined || value === "") {
    return `<c r="${reference}" s="${style}"/>`;
  }
  if (typeof value === "number" && Number.isFinite(value)) {
    return `<c r="${reference}" s="${STYLE.number}"><v>${value}</v></c>`;
  }
  return `<c r="${reference}" s="${style}" t="inlineStr"><is><t xml:space="preserve">${escapeXml(
    String(value),
  )}</t></is></c>`;
}

function rowXml(rowIndex: number, values: XlsxCell[], style: number, width: number): string {
  const cells: string[] = [];
  for (let index = 0; index < width; index += 1) {
    const reference = `${columnName(index)}${rowIndex}`;
    cells.push(cellXml(reference, values[index], style));
  }
  return `<row r="${rowIndex}">${cells.join("")}</row>`;
}

function widthsFor(sheet: XlsxSheet): number[] {
  const declaredWidth = sheet.header.length;
  const rowWidth = sheet.rows.reduce((max, row) => Math.max(max, row.length), 0);
  const width = Math.max(declaredWidth, rowWidth, 1);
  const resolved: number[] = [];
  for (let index = 0; index < width; index += 1) {
    const declared = sheet.columnWidths?.[index];
    if (typeof declared === "number" && declared > 0) {
      resolved.push(declared);
      continue;
    }
    // 表头/文本列宽一些，纯数字列窄一些；上限 60 避免一列撑满屏幕。
    const isNumericColumn =
      sheet.rows.length > 0 && sheet.rows.every((row) => typeof row[index] === "number");
    const headerCell = sheet.header[index] ?? "";
    resolved.push(isNumericColumn ? 14 : Math.min(60, Math.max(12, headerCell.length + 6)));
  }
  return resolved;
}

function sheetXml(sheet: XlsxSheet): string {
  const widths = widthsFor(sheet);
  const totalWidth = widths.length;
  const lastRow = Math.max(sheet.rows.length + 1, 1);
  const dimension = `A1:${columnName(totalWidth - 1)}${lastRow}`;
  const cols = widths
    .map(
      (width, index) =>
        `<col min="${index + 1}" max="${index + 1}" width="${width}" customWidth="1"/>`,
    )
    .join("");
  const rows: string[] = [rowXml(1, sheet.header, STYLE.header, totalWidth)];
  sheet.rows.forEach((values, index) => {
    rows.push(rowXml(index + 2, values, STYLE.default, totalWidth));
  });
  return (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>' +
    '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">' +
    `<dimension ref="${dimension}"/>` +
    '<sheetViews><sheetView workbookViewId="0"><pane ySplit="1" topLeftCell="A2" activePane="bottomLeft" state="frozen"/>' +
    '<selection pane="bottomLeft" activeCell="A2" sqref="A2"/></sheetView></sheetViews>' +
    '<sheetFormatPr defaultRowHeight="15"/>' +
    `<cols>${cols}</cols>` +
    `<sheetData>${rows.join("")}</sheetData>`
    + "</worksheet>"
  );
}

function contentTypesXml(sheetCount: number): string {
  const overrides: string[] = [];
  for (let index = 1; index <= sheetCount; index += 1) {
    overrides.push(
      `<Override PartName="/xl/worksheets/sheet${index}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>`,
    );
  }
  return (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>' +
    '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">' +
    '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>' +
    '<Default Extension="xml" ContentType="application/xml"/>' +
    '<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>' +
    '<Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>' +
    `${overrides.join("")}</Types>`
  );
}

function rootRelsXml(): string {
  return (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>' +
    '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">' +
    '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>' +
    "</Relationships>"
  );
}

function workbookXml(sheets: XlsxSheet[]): string {
  const entries = sheets
    .map(
      (sheet, index) =>
        `<sheet name="${escapeXml(sheetName(sheet.name))}" sheetId="${index + 1}" r:id="rId${index + 1}"/>`,
    )
    .join("");
  return (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>' +
    '<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" ' +
    'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">' +
    `<sheets>${entries}</sheets></workbook>`
  );
}

function workbookRelsXml(sheetCount: number): string {
  const entries: string[] = [];
  for (let index = 1; index <= sheetCount; index += 1) {
    entries.push(
      `<Relationship Id="rId${index}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet${index}.xml"/>`,
    );
  }
  entries.push(
    '<Relationship Id="rIdStyles" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>',
  );
  return (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>' +
    `<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">${entries.join("")}</Relationships>`
  );
}

function stylesXml(): string {
  return (
    '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>' +
    '<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">' +
    '<numFmts count="1"><numFmt numFmtId="164" formatCode="#,##0"/></numFmts>' +
    '<fonts count="2"><font><sz val="11"/><name val="Calibri"/></font><font><b/><sz val="11"/><name val="Calibri"/></font></fonts>' +
    '<fills count="3"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill>' +
    '<fill><patternFill patternType="solid"><fgColor rgb="FFEDF1F7"/><bgColor indexed="64"/></patternFill></fill></fills>' +
    '<borders count="1"><border/></borders>' +
    '<cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>' +
    '<cellXfs count="4"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/>' +
    '<xf numFmtId="0" fontId="1" fillId="2" borderId="0" xfId="0" applyFont="1" applyFill="1" applyAlignment="1">' +
    '<alignment horizontal="left" vertical="center"/></xf>' +
    '<xf numFmtId="164" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1" applyAlignment="1">' +
    '<alignment horizontal="right"/></xf>' +
    '<xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0" applyAlignment="1">' +
    '<alignment horizontal="left"/></xf></cellXfs>' +
    '<cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles></styleSheet>'
  );
}

// ---------------------------------------------------------------------------
// ZIP（store，无压缩）
// ---------------------------------------------------------------------------

const CRC_TABLE: number[] = (() => {
  const table: number[] = [];
  for (let index = 0; index < 256; index += 1) {
    let value = index;
    for (let bit = 0; bit < 8; bit += 1) {
      value = value & 1 ? 0xedb88320 ^ (value >>> 1) : value >>> 1;
    }
    table.push(value >>> 0);
  }
  return table;
})();

/** 标准 CRC-32（IEEE 802.3），返回无符号 32 位。 */
export function crc32(bytes: Uint8Array): number {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc = (CRC_TABLE[(crc ^ byte) & 0xff] ?? 0) ^ (crc >>> 8);
  }
  return (crc ^ 0xffffffff) >>> 0;
}

export interface ZipEntry {
  name: string;
  data: Uint8Array;
}

/** 组装一个最小 ZIP 包（store 模式，无压缩、无加密、无注释）。 */
export function buildZip(entries: ZipEntry[]): Uint8Array {
  const encoder = new TextEncoder();
  const locals: Uint8Array[] = [];
  const centrals: Uint8Array[] = [];
  let offset = 0;

  for (const entry of entries) {
    const name = encoder.encode(entry.name);
    const crc = crc32(entry.data);
    const local = new Uint8Array(30 + name.length);
    const localView = new DataView(local.buffer);
    localView.setUint32(0, 0x04034b50, true); // local file header
    localView.setUint16(4, 20, true); // version needed
    localView.setUint16(6, 0, true); // flags
    localView.setUint16(8, 0, true); // method: store
    localView.setUint16(10, 0, true); // mod time
    localView.setUint16(12, 0x21, true); // mod date (1980-01-01)
    localView.setUint32(14, crc, true);
    localView.setUint32(18, entry.data.length, true);
    localView.setUint32(22, entry.data.length, true);
    localView.setUint16(26, name.length, true);
    localView.setUint16(28, 0, true); // extra length
    local.set(name, 30);
    locals.push(local, entry.data);

    const central = new Uint8Array(46 + name.length);
    const centralView = new DataView(central.buffer);
    centralView.setUint32(0, 0x02014b50, true); // central directory header
    centralView.setUint16(4, 20, true); // version made by
    centralView.setUint16(6, 20, true); // version needed
    centralView.setUint16(8, 0, true); // flags
    centralView.setUint16(10, 0, true); // method: store
    centralView.setUint16(12, 0, true);
    centralView.setUint16(14, 0x21, true);
    centralView.setUint32(16, crc, true);
    centralView.setUint32(20, entry.data.length, true);
    centralView.setUint32(24, entry.data.length, true);
    centralView.setUint16(28, name.length, true);
    centralView.setUint16(30, 0, true); // extra
    centralView.setUint16(32, 0, true); // comment
    centralView.setUint16(34, 0, true); // disk start
    centralView.setUint16(36, 0, true); // internal attrs
    centralView.setUint32(38, 0, true); // external attrs
    centralView.setUint32(42, offset, true); // local header offset
    central.set(name, 46);
    centrals.push(central);

    offset += local.length + entry.data.length;
  }

  const centralSize = centrals.reduce((sum, item) => sum + item.length, 0);
  const end = new Uint8Array(22);
  const endView = new DataView(end.buffer);
  endView.setUint32(0, 0x06054b50, true); // end of central directory
  endView.setUint16(4, 0, true);
  endView.setUint16(6, 0, true);
  endView.setUint16(8, entries.length, true);
  endView.setUint16(10, entries.length, true);
  endView.setUint32(12, centralSize, true);
  endView.setUint32(16, offset, true);
  endView.setUint16(20, 0, true);

  const total = locals.reduce((sum, item) => sum + item.length, 0) + centralSize + end.length;
  const output = new Uint8Array(total);
  let cursor = 0;
  for (const item of [...locals, ...centrals, end]) {
    output.set(item, cursor);
    cursor += item.length;
  }
  return output;
}

/**
 * 生成 xlsx 字节。
 *
 * 每个 `XlsxSheet` 一个工作表；首行是表头（冻结 + 加粗 + 浅底），数据行按值类型
 * 分别写成数字/文本。
 */
export function buildXlsx(sheets: XlsxSheet[]): Uint8Array {
  if (sheets.length === 0) {
    throw new Error("xlsx 至少需要一个工作表");
  }
  const encoder = new TextEncoder();
  const entries: ZipEntry[] = [
    { name: "[Content_Types].xml", data: encoder.encode(contentTypesXml(sheets.length)) },
    { name: "_rels/.rels", data: encoder.encode(rootRelsXml()) },
    { name: "xl/workbook.xml", data: encoder.encode(workbookXml(sheets)) },
    { name: "xl/_rels/workbook.xml.rels", data: encoder.encode(workbookRelsXml(sheets.length)) },
    { name: "xl/styles.xml", data: encoder.encode(stylesXml()) },
  ];
  sheets.forEach((sheet, index) => {
    entries.push({
      name: `xl/worksheets/sheet${index + 1}.xml`,
      data: encoder.encode(sheetXml(sheet)),
    });
  });
  return buildZip(entries);
}

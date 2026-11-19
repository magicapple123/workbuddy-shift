import { describe, expect, it } from "vitest";

import { crc32, buildXlsx, columnName, sheetName, buildZip } from "./xlsx";

/** 解析 store 模式 ZIP 的全部条目：返回 name → UTF-8 文本。 */
export function readStoredZip(bytes: Uint8Array): Map<string, string> {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const decoder = new TextDecoder();
  const entries = new Map<string, string>();
  let cursor = 0;
  while (cursor + 30 <= bytes.length) {
    if (view.getUint32(cursor, true) !== 0x04034b50) break;
    const method = view.getUint16(cursor + 8, true);
    const compressedSize = view.getUint32(cursor + 18, true);
    const nameLength = view.getUint16(cursor + 26, true);
    const extraLength = view.getUint16(cursor + 28, true);
    const nameStart = cursor + 30;
    const name = decoder.decode(bytes.subarray(nameStart, nameStart + nameLength));
    const dataStart = nameStart + nameLength + extraLength;
    const data = bytes.subarray(dataStart, dataStart + compressedSize);
    // 只实现了 store：遇到压缩条目直接失败，避免测试静默通过。
    expect(method).toBe(0);
    entries.set(name, decoder.decode(data));
    cursor = dataStart + compressedSize;
  }
  return entries;
}

describe("xlsx 生成器", () => {
  it("crc32 与标准实现一致", () => {
    expect(crc32(new TextEncoder().encode("123456789"))).toBe(0xcbf43926);
  });

  it("列名与 sheet 名符合 Excel 约束", () => {
    expect(columnName(0)).toBe("A");
    expect(columnName(25)).toBe("Z");
    expect(columnName(26)).toBe("AA");
    expect(sheetName("按会话")).toBe("按会话");
    expect(sheetName("a[b]:c*d?e/f\\g")).toBe("a-b--c-d-e-f-g");
    expect(sheetName("x".repeat(40)).length).toBe(31);
    expect(sheetName("")).toBe("Sheet");
  });

  it("ZIP 条目可原样读回", () => {
    const encoder = new TextEncoder();
    const zip = buildZip([{ name: "a.txt", data: encoder.encode("hello") }]);
    expect(readStoredZip(zip).get("a.txt")).toBe("hello");
  });

  it("工作簿包含 7 个 sheet 的结构文件", () => {
    const bytes = buildXlsx([
      { name: "汇总", header: ["指标", "数值"], rows: [["总 Token", 1064789123]] },
      { name: "按日", header: ["日期", "总 Token"], rows: [["2026-11-19", 12]] },
    ]);
    const entries = readStoredZip(bytes);
    expect([...entries.keys()].sort()).toEqual([
      "[Content_Types].xml",
      "_rels/.rels",
      "xl/_rels/workbook.xml.rels",
      "xl/styles.xml",
      "xl/workbook.xml",
      "xl/worksheets/sheet1.xml",
      "xl/worksheets/sheet2.xml",
    ]);
    expect(entries.get("xl/workbook.xml")).toContain('name="汇总"');
    expect(entries.get("xl/workbook.xml")).toContain('name="按日"');
    expect(entries.get("[Content_Types].xml")).toContain("/xl/worksheets/sheet2.xml");
  });

  it("首行冻结、表头有样式、数字是数值类型（不是文本）", () => {
    const bytes = buildXlsx([
      { name: "汇总", header: ["指标", "数值"], rows: [["总 Token", 1064789123]] },
    ]);
    const sheet = readStoredZip(bytes).get("xl/worksheets/sheet1.xml") ?? "";
    expect(sheet).toContain('state="frozen"');
    expect(sheet).toContain('ySplit="1"');
    // 表头用样式 1（加粗 + 浅底），对齐方式在 styles.xml 里。
    expect(sheet).toContain('<c r="A1" s="1" t="inlineStr">');
    // 数字写 <v>，可被 Excel 求和；也没有被引号包成文本。
    expect(sheet).toContain('<c r="B2" s="2"><v>1064789123</v></c>');
    expect(sheet).not.toContain('"1064789123"');
    // 列宽由内容估算，数值列 14。
    expect(sheet).toContain('<col min="2" max="2" width="14" customWidth="1"/>');
  });

  it("空表与空值不产生非法单元格", () => {
    const bytes = buildXlsx([{ name: "空", header: ["a", "b"], rows: [] }]);
    const sheet = readStoredZip(bytes).get("xl/worksheets/sheet1.xml") ?? "";
    expect(sheet).toContain('<dimension ref="A1:B1"/>');
    expect(sheet).not.toContain("undefined");
    expect(sheet).not.toContain("null");
  });
});

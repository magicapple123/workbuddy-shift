import { describe, expect, it } from "vitest";

import { CSV_BOM, buildCsv, csvFileName, escapeCsvCell } from "./csv";

describe("CSV 基础能力", () => {
  it("含逗号/引号/换行的单元格加引号并双写内部引号", () => {
    expect(escapeCsvCell("a,b")).toBe('"a,b"');
    expect(escapeCsvCell('say "hi"')).toBe('"say ""hi"""');
    expect(escapeCsvCell("l1\nl2")).toBe('"l1\nl2"');
    expect(escapeCsvCell("plain")).toBe("plain");
  });

  it("公式注入被前置单引号，数字不受影响", () => {
    expect(escapeCsvCell("=SUM(A1:A2)")).toBe("'=SUM(A1:A2)");
    expect(escapeCsvCell("+1")).toBe("'+1");
    expect(escapeCsvCell("-1")).toBe("'-1");
    expect(escapeCsvCell("@cmd")).toBe("'@cmd");
    expect(escapeCsvCell(-1)).toBe("-1");
    expect(escapeCsvCell(0)).toBe("0");
  });

  it("文件以 BOM 开头、用 CRLF、区块之间空一行", () => {
    const text = buildCsv([
      { title: "区块一", headers: ["a", "b"], rows: [[1, 2]] },
      { title: "区块二", headers: ["c"], rows: [["x"]] },
    ]);
    expect(text.startsWith(CSV_BOM)).toBe(true);
    expect(text).toContain("\r\n");
    expect(text).not.toMatch(/[^\r]\n/);
    const lines = text.slice(CSV_BOM.length).split("\r\n");
    expect(lines[0]).toBe("区块一");
    expect(lines.slice(0, 5)).toEqual(["区块一", "a,b", "1,2", "", "区块二"]);
  });

  it("区块标题以 = 开头时导出后不是公式", () => {
    const text = buildCsv([{ title: "=cmd|calc", headers: ["a"], rows: [["=1+1"]] }]);
    const body = text.slice(CSV_BOM.length);
    expect(body.startsWith("'=cmd|calc")).toBe(true);
    expect(body).toContain("'=1+1");
    const rows = body.split("\r\n");
    expect(rows.some((row) => row.startsWith("="))).toBe(false);
  });

  it("文件名带时间戳且以 .csv 结尾", () => {
    const name = csvFileName("token-stats", "workbuddy", new Date(2026, 10, 17, 9, 5));
    expect(name).toBe("token-stats-workbuddy-20261117-0905.csv");
  });
});

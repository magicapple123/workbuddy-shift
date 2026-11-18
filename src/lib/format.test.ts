import { describe, expect, it } from "vitest";

import {
  EMPTY_TEXT,
  dateDaysAgo,
  dateKey,
  formatAmount,
  formatDate,
  formatDateTime,
  formatFullDateTime,
  isUsableTimestamp,
  pad2,
} from "./format";

describe("format", () => {
  it("pad2 补零", () => {
    expect(pad2(0)).toBe("00");
    expect(pad2(7)).toBe("07");
    expect(pad2(23)).toBe("23");
  });

  it("isUsableTimestamp 把 0 / 负数 / 非有限值视为缺失", () => {
    expect(isUsableTimestamp(1)).toBe(true);
    expect(isUsableTimestamp(0)).toBe(false);
    expect(isUsableTimestamp(-1)).toBe(false);
    expect(isUsableTimestamp(Number.NaN)).toBe(false);
    expect(isUsableTimestamp(Number.POSITIVE_INFINITY)).toBe(false);
    expect(isUsableTimestamp(null)).toBe(false);
    expect(isUsableTimestamp(undefined)).toBe(false);
  });

  it("dateKey 输出本地 YYYY-MM-DD", () => {
    expect(dateKey(new Date(2026, 0, 5))).toBe("2026-01-05");
    expect(dateKey(new Date(2026, 10, 16))).toBe("2026-11-16");
  });

  it("dateDaysAgo 锚在中午，逐次调用不跳日", () => {
    const today = dateKey(new Date());
    expect(dateDaysAgo(0)).toBe(today);
    expect(today.localeCompare(dateDaysAgo(1))).toBeGreaterThan(0);
  });

  it("时间格式化对 0 / null / 非法值返回占位符，可用时间戳不返回占位符", () => {
    expect(formatDate(0)).toBe(EMPTY_TEXT);
    expect(formatDate(null)).toBe(EMPTY_TEXT);
    expect(formatDateTime(undefined, "-")).toBe("-");
    expect(formatFullDateTime(-1)).toBe(EMPTY_TEXT);
    const ts = Date.UTC(2024, 0, 2, 3, 4);
    expect(formatDate(ts)).not.toBe(EMPTY_TEXT);
    expect(formatDateTime(ts)).not.toBe(EMPTY_TEXT);
    expect(formatFullDateTime(ts)).not.toBe(EMPTY_TEXT);
  });

  it("formatAmount 千分位 + 最多两位小数，非有限值回退占位符", () => {
    expect(formatAmount(1234567.891)).toBe("1,234,567.89");
    expect(formatAmount(1)).toBe("1");
    expect(formatAmount(Number.NaN)).toBe(EMPTY_TEXT);
    expect(formatAmount(null, "无")).toBe("无");
  });
});

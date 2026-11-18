import { describe, expect, it } from "vitest";

import {
  CHECKIN_NUMBER_FIELDS,
  ROTATE_NUMBER_FIELDS,
  checkinWindowIssue,
  logLabel,
  numberInputHint,
  resolveNumberInput,
} from "./helpers";

describe("settings helpers", () => {
  it("logLabel 映射签到结果文案与色调", () => {
    expect(logLabel("success")).toEqual({ text: "签到成功", tone: "success" });
    expect(logLabel("already")).toEqual({ text: "已签到", tone: "warning" });
    expect(logLabel("error")).toEqual({ text: "失败", tone: "error" });
    expect(logLabel("unexpected")).toEqual({ text: "失败", tone: "error" });
  });

  it("checkinWindowIssue：两端都空或都合法时返回 null", () => {
    expect(checkinWindowIssue("", "")).toBeNull();
    expect(checkinWindowIssue("08:00", "09:30")).toBeNull();
    expect(checkinWindowIssue("0:05", "23:59")).toBeNull();
  });

  it("checkinWindowIssue：半填 / 非法格式 / 结束不晚于开始都给出未保存说明", () => {
    expect(checkinWindowIssue("08:00", "")).toContain("需同时选择");
    expect(checkinWindowIssue("", "09:00")).toContain("需同时选择");
    expect(checkinWindowIssue("8点", "09:00")).toContain("HH:MM");
    expect(checkinWindowIssue("24:00", "23:00")).toContain("HH:MM");
    expect(checkinWindowIssue("09:00", "09:00")).toContain("结束时间需晚于开始时间");
    expect(checkinWindowIssue("10:00", "09:00")).toContain("不支持跨午夜");
  });

  it("resolveNumberInput：空 / 非数字返回 null，数字取整收敛到边界", () => {
    const field = CHECKIN_NUMBER_FIELDS.keepalive_days;
    expect(resolveNumberInput("", field)).toBeNull();
    expect(resolveNumberInput("abc", field)).toBeNull();
    expect(resolveNumberInput("7", field)).toBe(7);
    expect(resolveNumberInput("7.6", field)).toBe(8);
    expect(resolveNumberInput("-5", field)).toBe(0);
    expect(resolveNumberInput("999", field)).toBe(90);
  });

  it("resolveNumberInput：无上限字段只收敛下界", () => {
    const field = ROTATE_NUMBER_FIELDS.min_remaining_credits;
    expect(resolveNumberInput("-1", field)).toBe(0);
    expect(resolveNumberInput("99999", field)).toBe(99999);
  });

  it("numberInputHint 按是否有上限给出不同提示", () => {
    expect(numberInputHint(CHECKIN_NUMBER_FIELDS.keepalive_days)).toContain("0–90");
    expect(numberInputHint(ROTATE_NUMBER_FIELDS.min_remaining_credits)).toContain("不小于 0");
  });
});

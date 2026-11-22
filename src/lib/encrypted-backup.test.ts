import { describe, expect, it, vi } from "vitest";

import {
  MIN_PASSWORD_LENGTH,
  exportPasswordError,
  importPasswordError,
  importSummary,
  passwordLengthError,
  submitEncryptedExport,
  submitEncryptedImport,
} from "./encrypted-backup";

/**
 * 加密备份（issue #186）纯逻辑用例。
 *
 * 两条主线：
 *   1. 口令策略与前端口径一致（不足 8 位、两次输入不一致都必须拦在提交之前）；
 *   2. 校验没过时**命令一次都没下发**——用注入的假 `run` 直接断言调用次数，
 *      不依赖渲染（仓库没有 testing-library）。
 */

const GOOD_PASSWORD = "correct horse battery staple";

describe("encrypted backup password policy", () => {
  it("passwordLengthError 以 8 位为下限并按字符计数", () => {
    expect(MIN_PASSWORD_LENGTH).toBe(8);
    expect(passwordLengthError("")).toContain("至少 8 位");
    expect(passwordLengthError("1234567")).toContain("至少 8 位");
    expect(passwordLengthError("12345678")).toBeNull();
    // 中文按字符算：8 个汉字通过，4 个汉字不通过（否则会按字节数误判成 12 位）。
    expect(passwordLengthError("口令口令口令口令")).toBeNull();
    expect(passwordLengthError("口令口令")).toContain("至少 8 位");
  });

  it("passwordLengthError 的文案带当前位数但不回显口令", () => {
    const message = passwordLengthError("1234");
    expect(message).toBe("密码至少 8 位（当前 4 位）");
    expect(message).not.toContain("1234");
  });

  it("exportPasswordError 拦下两次输入不一致", () => {
    expect(exportPasswordError(GOOD_PASSWORD, GOOD_PASSWORD)).toBeNull();
    expect(exportPasswordError(GOOD_PASSWORD, `${GOOD_PASSWORD}x`)).toBe("两次输入的密码不一致");
    // 两次都为空时先报长度：空口令是最常见的漏填，报"不一致"会让人以为填对了。
    expect(exportPasswordError("", "")).toContain("至少 8 位");
  });

  it("importPasswordError 只校验长度（导入没有二次确认）", () => {
    expect(importPasswordError("")).toContain("至少 8 位");
    expect(importPasswordError("1234567")).toContain("至少 8 位");
    expect(importPasswordError("12345678")).toBeNull();
  });
});

describe("encrypted export submit", () => {
  it("未填口令不发请求、也不弹保存对话框", async () => {
    const run = vi.fn();
    const pickPath = vi.fn(async () => "/tmp/never.json");

    const outcome = await submitEncryptedExport("", "", { pickPath, run });

    expect(outcome).toEqual({ status: "invalid", error: expect.stringContaining("至少 8 位") });
    expect(run).not.toHaveBeenCalled();
    expect(pickPath).not.toHaveBeenCalled();
  });

  it("口令不足 8 位或两次不一致时不发请求", async () => {
    const run = vi.fn();
    const pickPath = vi.fn(async () => "/tmp/never.json");

    const short = await submitEncryptedExport("short", "short", { pickPath, run });
    expect(short.status).toBe("invalid");
    const mismatch = await submitEncryptedExport(GOOD_PASSWORD, `${GOOD_PASSWORD}!`, {
      pickPath,
      run,
    });
    expect(mismatch).toEqual({ status: "invalid", error: "两次输入的密码不一致" });
    expect(run).not.toHaveBeenCalled();
    expect(pickPath).not.toHaveBeenCalled();
  });

  it("用户在保存对话框取消时不下发命令", async () => {
    const run = vi.fn();
    const outcome = await submitEncryptedExport(GOOD_PASSWORD, GOOD_PASSWORD, {
      pickPath: async () => null,
      run,
    });

    expect(outcome).toEqual({ status: "canceled" });
    expect(run).not.toHaveBeenCalled();
  });

  it("校验通过后把路径与口令原样下发给命令，并带回账号数", async () => {
    const run = vi.fn(async (path: string, _password: string) => ({
      ok: true,
      path,
      count: 3,
    }));

    const outcome = await submitEncryptedExport(GOOD_PASSWORD, GOOD_PASSWORD, {
      pickPath: async () => "/tmp/backup.json",
      run,
    });

    expect(run).toHaveBeenCalledWith("/tmp/backup.json", GOOD_PASSWORD);
    expect(outcome).toEqual({ status: "done", path: "/tmp/backup.json", count: 3 });
  });

  it("后端拒绝时返回 failed 并把原始错误交给调用方", async () => {
    const error = new Error("目标目录不存在：/nope");
    const outcome = await submitEncryptedExport(GOOD_PASSWORD, GOOD_PASSWORD, {
      pickPath: async () => "/nope/backup.json",
      run: async () => {
        throw error;
      },
    });

    expect(outcome).toEqual({ status: "failed", error });
  });
});

describe("encrypted import submit", () => {
  it("未填口令不发请求、也不弹打开对话框", async () => {
    const run = vi.fn();
    const pickPath = vi.fn(async () => "/tmp/never.json");

    const outcome = await submitEncryptedImport("", { pickPath, run });

    expect(outcome).toEqual({ status: "invalid", error: expect.stringContaining("至少 8 位") });
    expect(run).not.toHaveBeenCalled();
    expect(pickPath).not.toHaveBeenCalled();
  });

  it("取消选文件时不下发命令", async () => {
    const run = vi.fn();
    const outcome = await submitEncryptedImport(GOOD_PASSWORD, {
      pickPath: async () => null,
      run,
    });

    expect(outcome).toEqual({ status: "canceled" });
    expect(run).not.toHaveBeenCalled();
  });

  it("成功时带回导入摘要（导入 / 覆盖 / 跳过）", async () => {
    const run = vi.fn(async () => ({ ok: true, imported: 3, skipped: 1, overwritten: 2 }));

    const outcome = await submitEncryptedImport(GOOD_PASSWORD, {
      pickPath: async () => "/tmp/backup.json",
      run,
    });

    expect(run).toHaveBeenCalledWith("/tmp/backup.json", GOOD_PASSWORD);
    expect(outcome.status).toBe("done");
    if (outcome.status === "done") {
      expect(importSummary(outcome.report)).toBe("新增 1 个，覆盖 2 个，跳过 1 个");
    }
  });

  it("wrong password 这类后端错误保持为 failed（由弹窗展示可读文案）", async () => {
    const outcome = await submitEncryptedImport(GOOD_PASSWORD, {
      pickPath: async () => "/tmp/backup.json",
      run: async () => {
        throw "解密失败：密码错误或文件已损坏";
      },
    });

    expect(outcome).toEqual({ status: "failed", error: "解密失败：密码错误或文件已损坏" });
  });
});

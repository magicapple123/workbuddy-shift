import { beforeEach, describe, expect, it, vi } from "vitest";

const saveBytesFile = vi.fn(async () => ({
  ok: true,
  path: "C:/tmp/a.xlsx",
  bytes: 3,
  savedAt: 0,
}));
const dialogSave = vi.fn(async (): Promise<string | null> => "C:/tmp/a.xlsx");

vi.mock("@/lib/api", () => ({
  isWebui: () => false,
  isDemoMode: () => false,
  saveBytesFile: (...args: unknown[]) => saveBytesFile(...(args as [])),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  save: (...args: unknown[]) => dialogSave(...(args as [])),
}));

import { saveXlsxFile } from "./export-download";

describe("saveXlsxFile（桌面）", () => {
  beforeEach(() => {
    saveBytesFile.mockClear();
    dialogSave.mockClear();
    dialogSave.mockResolvedValue("C:/tmp/a.xlsx");
  });

  it("走保存对话框 + save_bytes_file，内容按 base64 传入", async () => {
    const bytes = new Uint8Array([1, 2, 3]);
    const result = await saveXlsxFile("Token 明细.xlsx", bytes);

    expect(result).toBe("saved");
    expect(dialogSave).toHaveBeenCalledTimes(1);
    expect(saveBytesFile).toHaveBeenCalledTimes(1);
    const [path, base64] = saveBytesFile.mock.calls[0] as unknown as [string, string];
    expect(path).toBe("C:/tmp/a.xlsx");
    expect(base64).toBe(btoa(String.fromCharCode(1, 2, 3)));
  });

  it("用户取消对话框时不写文件", async () => {
    dialogSave.mockResolvedValueOnce(null);
    const result = await saveXlsxFile("Token 明细.xlsx", new Uint8Array([1]));

    expect(result).toBe("cancelled");
    expect(saveBytesFile).not.toHaveBeenCalled();
  });
});

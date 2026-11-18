import * as api from "@/lib/api";
import { isWebui } from "@/lib/api";

/**
 * 保存 CSV：WebUI 走浏览器下载，桌面走保存对话框 + `save_text_file`。
 *
 * 返回 `cancelled` 表示用户在对话框里取消；其它错误向上抛，由调用方提示。
 */
export async function saveCsvFile(fileName: string, content: string): Promise<"saved" | "cancelled"> {
  if (isWebui() || api.isDemoMode()) {
    const blob = new Blob([content], { type: "text/csv;charset=utf-8" });
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = fileName;
    document.body.appendChild(anchor);
    anchor.click();
    anchor.remove();
    // 交给浏览器读完再释放，避免下载被取消。
    setTimeout(() => URL.revokeObjectURL(url), 4000);
    return "saved";
  }
  const { save } = await import("@tauri-apps/plugin-dialog");
  const target = await save({
    defaultPath: fileName,
    filters: [{ name: "CSV", extensions: ["csv"] }],
  });
  if (!target) return "cancelled";
  await api.saveTextFile(target, content);
  return "saved";
}

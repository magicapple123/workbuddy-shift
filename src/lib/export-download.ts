import * as api from "@/lib/api";
import { isWebui } from "@/lib/api";

/**
 * 保存 CSV：WebUI 走浏览器下载，桌面走保存对话框 + `save_text_file`。
 *
 * 返回 `cancelled` 表示用户在对话框里取消；其它错误向上抛，由调用方提示。
 */
export async function saveCsvFile(fileName: string, content: string): Promise<"saved" | "cancelled"> {
  if (isWebui() || api.isDemoMode()) {
    downloadBlob(fileName, new Blob([content], { type: "text/csv;charset=utf-8" }));
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

const XLSX_MIME = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

/** 发起一次 Blob 下载（WebUI 浏览器与桌面 webview 共用同一段代码）。 */
function downloadBlob(fileName: string, blob: Blob): void {
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = fileName;
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  // 交给浏览器读完再释放，避免下载被取消。
  setTimeout(() => URL.revokeObjectURL(url), 4000);
}

/**
 * 保存 xlsx：二进制在前端生成，两种宿主都走 Blob 下载（清单第 17 条决策）。
 *
 * 为什么不复用 `save_text_file`：那条命令只允许 `.csv`，且按 UTF-8 文本写，装不下
 * ZIP 二进制；新增一条二进制写入命令会让 WebUI 多开一条通道（该决策明确不做），
 * 所以这里统一走 Blob。
 */
export async function saveXlsxFile(
  fileName: string,
  bytes: Uint8Array,
): Promise<"saved" | "cancelled"> {
  // `bytes.buffer` 可能是 SharedArrayBuffer 视图，拷一份独立的 ArrayBuffer 交给 Blob。
  const buffer = bytes.buffer.slice(
    bytes.byteOffset,
    bytes.byteOffset + bytes.byteLength,
  ) as ArrayBuffer;
  downloadBlob(fileName, new Blob([buffer], { type: XLSX_MIME }));
  return "saved";
}

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
 * 保存 xlsx：二进制在前端生成。
 *
 * - WebUI / 演示模式：浏览器 Blob 下载；
 * - 桌面：先弹保存对话框，再调 desktop-only 的 `save_bytes_file` 原子写入。
 *   （WebView2 里 Blob 下载不会落盘，2026-11-19 实机确认；因此桌面必须走命令。）
 */
export async function saveXlsxFile(
  fileName: string,
  bytes: Uint8Array,
): Promise<"saved" | "cancelled"> {
  if (isWebui() || api.isDemoMode()) {
    // `bytes.buffer` 可能是 SharedArrayBuffer 视图，拷一份独立的 ArrayBuffer 交给 Blob。
    const buffer = bytes.buffer.slice(
      bytes.byteOffset,
      bytes.byteOffset + bytes.byteLength,
    ) as ArrayBuffer;
    downloadBlob(fileName, new Blob([buffer], { type: XLSX_MIME }));
    return "saved";
  }
  const { save } = await import("@tauri-apps/plugin-dialog");
  const target = await save({
    defaultPath: fileName,
    filters: [{ name: "Excel", extensions: ["xlsx"] }],
  });
  if (!target) return "cancelled";
  await api.saveBytesFile(target, toBase64(bytes));
  return "saved";
}

/** 分块转 base64，避免 `String.fromCharCode(...大数组)` 触发调用栈上限。 */
function toBase64(bytes: Uint8Array): string {
  const chunkSize = 0x8000;
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += chunkSize) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + chunkSize));
  }
  return btoa(binary);
}

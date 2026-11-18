// 本项目自己的发布仓库（与 Rust 侧 `update.rs` 的 GITHUB_OWNER/GITHUB_REPO、以及
// `src-tauri/tauri.conf.json` 的 updater endpoints 三处必须一致）。
// 注意不要指回上游仓库：那会让"查看仓库/下载更新"把用户带回原版（迁移逻辑见 update.rs）。
export const GITHUB_OWNER = "magicapple123";
export const GITHUB_REPO = "workbuddy-shift";
export const GITHUB_REPOSITORY_URL = `https://github.com/${GITHUB_OWNER}/${GITHUB_REPO}`;
export const GITHUB_RELEASE_URL = `${GITHUB_REPOSITORY_URL}/releases/latest`;

/** 在桌面端通过 Tauri opener 打开，在 webui 端打开新标签页。 */
export async function openReleaseUrl(url = GITHUB_RELEASE_URL): Promise<void> {
  const isWebui = typeof window !== "undefined" && !("__TAURI_INTERNALS__" in window);
  if (isWebui) {
    window.open(url, "_blank", "noopener,noreferrer");
    return;
  }
  const { openUrl } = await import("@tauri-apps/plugin-opener");
  await openUrl(url);
}

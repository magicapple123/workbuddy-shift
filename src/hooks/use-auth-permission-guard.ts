import { useEffect, useState } from "react";

import * as api from "@/lib/api";
import type { WbVariant } from "@/lib/types";

interface Options {
  open: boolean;
  error: string;
  variant: WbVariant;
  /** 打开授权设置 / 轮询失败时把错误写回弹窗。 */
  onError: (error: string) => void;
}

/**
 * 完全磁盘访问授权引导：权限自检、打开系统设置，以及出现「无权限」错误时每 2s 自动轮询。
 *
 * 拖入 app 授权成功后轮询会自动清除错误并给出成功提示，用户无需重新点切换。
 */
export function useAuthPermissionGuard({ open, error, variant, onError }: Options) {
  const [permCheck, setPermCheck] = useState<string | null>(null);

  async function openPermissionSettings() {
  try {
    await api.openPermissionSettings("all_files");
  } catch (e) {
    // 打开失败时退化为提示
    onError(api.asError(e));
  }
  }

  async function runPermissionCheck() {
  setPermCheck("检测中…");
  try {
    const res = await api.checkAuthPermission(variant);
    setPermCheck(res.ok ? `✓ ${res.message}` : `✗ ${res.error}（${res.dir}）`);
  } catch (e) {
    setPermCheck(`✗ ${api.asError(e)}`);
  }
  }

  // 出现「无权限」错误时，自动每 2s 轮询一次授权状态；用户拖入 app 授权成功后自动恢复
  useEffect(() => {
  if (!open || !error.includes("无权限")) return;
  let cancelled = false;
  let timer: number | undefined;
  const check = async () => {
    try {
      const res = await api.checkAuthPermission(variant);
      if (res.ok) {
        if (!cancelled) {
          setPermCheck("✓ 授权成功，可以重新切换了");
          onError("");
        }
        return;
      }
    } catch {
      /* 忽略中间态 */
    }
    if (!cancelled) timer = window.setTimeout(check, 2000);
  };
  check();
  return () => {
    cancelled = true;
    if (timer) window.clearTimeout(timer);
  };
  }, [error, open, variant]);

  return { permCheck, setPermCheck, runPermissionCheck, openPermissionSettings };
}

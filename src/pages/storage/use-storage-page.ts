import { useCallback, useEffect, useRef, useState } from "react";
import { toast } from "sonner";

import * as api from "@/lib/api";
import type {
  StorageCleanPlan,
  StorageCleanResult,
  StorageCleanTarget,
  StorageScan,
  StorageSession,
  StorageTrashPayload,
} from "@/lib/types";

/** 外部删除重校验的防抖时长：页面 focus / 可见性变化后 3 秒才查一次。 */
const REVALIDATE_DEBOUNCE_MS = 3000;
/** 单次重校验的会话上限（core 侧硬上限 100，分类另算一次请求）。 */
const REVALIDATE_SESSIONS = 80;

interface UseStoragePage {
  scan: StorageScan | null;
  sessions: StorageSession[];
  trash: StorageTrashPayload | null;
  loading: boolean;
  busy: boolean;
  error: string | null;
  presence: Record<string, string>;
  lastRevalidatedAt: number | null;
  plan: StorageCleanPlan | null;
  dialogOpen: boolean;
  refresh: () => Promise<void>;
  previewClean: (targets: StorageCleanTarget[]) => Promise<void>;
  closeDialog: () => void;
  confirmClean: () => Promise<void>;
  restore: (id: string) => Promise<void>;
  purge: (id: string) => Promise<void>;
  purgeAll: () => Promise<void>;
  reveal: (path: string, mode: "select" | "open") => Promise<void>;
}

/**
 * 存储管理页的数据编排。
 *
 * - 首屏并行取扫描、会话资产与回收站；
 * - 清理固定走「dry-run → 确认 → 执行」，执行后重扫；
 * - 外部删除同步：focus / visibility 变化时防抖 3 秒，只校验当前一页（分类 + 前 80 条会话）。
 */
export function useStoragePage(): UseStoragePage {
  const [scan, setScan] = useState<StorageScan | null>(null);
  const [sessions, setSessions] = useState<StorageSession[]>([]);
  const [trash, setTrash] = useState<StorageTrashPayload | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [presence, setPresence] = useState<Record<string, string>>({});
  const [lastRevalidatedAt, setLastRevalidatedAt] = useState<number | null>(null);
  const [plan, setPlan] = useState<StorageCleanPlan | null>(null);
  const [pending, setPending] = useState<StorageCleanTarget[] | null>(null);
  const [dialogOpen, setDialogOpen] = useState(false);
  const sessionsRef = useRef<StorageSession[]>([]);
  sessionsRef.current = sessions;

  const refresh = useCallback(async () => {
    setError(null);
    try {
      const [nextScan, nextSessions, nextTrash] = await Promise.all([
        api.storageScan(),
        api.storageSessions(),
        api.storageTrash(),
      ]);
      setScan(nextScan);
      setSessions(nextSessions.sessions);
      setTrash(nextTrash);
      setPresence({});
    } catch (cause) {
      setError(api.asError(cause));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // 外部删除同步：不监听文件系统，只在用户切回页面时按当前一页校验。
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | null = null;
    const run = async () => {
      const current = sessionsRef.current;
      if (current.length === 0) return;
      const sessionTargets = current
        .slice(0, REVALIDATE_SESSIONS)
        .map((session) => `session:${session.id}`);
      const categoryTargets = (scan?.categories ?? []).map((category) => `category:${category.id}`);
      try {
        const [sessionResult, categoryResult] = await Promise.all([
          api.storageRevalidate(sessionTargets),
          categoryTargets.length > 0
            ? api.storageRevalidate(categoryTargets)
            : Promise.resolve({ items: [] }),
        ]);
        setPresence((previous) => {
          const next = { ...previous };
          for (const item of sessionResult.items) {
            const id = item.target.startsWith("session:") ? item.target.slice(8) : "";
            if (id && item.presence) next[id] = item.presence;
          }
          return next;
        });
        setScan((previous) => {
          if (!previous) return previous;
          const byId = new Map(categoryResult.items.map((item) => [item.target, item]));
          return {
            ...previous,
            categories: previous.categories.map((category) => {
              const item = byId.get(`category:${category.id}`);
              return item ? { ...category, bytes: item.bytes, files: item.files } : category;
            }),
          };
        });
        setLastRevalidatedAt(Date.now());
      } catch {
        // 重校验失败不打扰用户：保留上一次数字，下次 focus 再试。
      }
    };
    const schedule = () => {
      if (document.visibilityState !== "visible") return;
      if (timer) clearTimeout(timer);
      timer = setTimeout(() => {
        timer = null;
        void run();
      }, REVALIDATE_DEBOUNCE_MS);
    };
    window.addEventListener("focus", schedule);
    document.addEventListener("visibilitychange", schedule);
    return () => {
      if (timer) clearTimeout(timer);
      window.removeEventListener("focus", schedule);
      document.removeEventListener("visibilitychange", schedule);
    };
    // `scan.categories` 只用于拼 targets；分类数量变化时重新挂载监听没有副作用。
  }, [scan?.categories]);

  const previewClean = useCallback(async (targets: StorageCleanTarget[]) => {
    setBusy(true);
    setPlan(null);
    setPending(targets);
    setDialogOpen(true);
    try {
      const result = await api.storageClean(targets, true);
      if (result.dryRun) {
        setPlan(result);
      }
    } catch (cause) {
      setDialogOpen(false);
      setPending(null);
      toast.error("无法清理", { description: api.asError(cause) });
    } finally {
      setBusy(false);
    }
  }, []);

  const closeDialog = useCallback(() => {
    setDialogOpen(false);
    setPlan(null);
    setPending(null);
  }, []);

  const confirmClean = useCallback(async () => {
    if (!pending) return;
    setBusy(true);
    try {
      const result = (await api.storageClean(pending, false)) as StorageCleanResult;
      const failed = result.failures.length;
      if (failed > 0) {
        toast.warning(`已清理 ${result.moved} 项，${failed} 项失败`, {
          description: result.failures[0]?.error,
        });
      } else {
        toast.success(`已清理 ${result.moved} 项`, {
          description: `${formatMb(result.bytes)} 已移入回收站 ${
            result.deletedRows > 0 ? `· 删除 ${result.deletedRows} 条会话行` : ""
          }`,
        });
      }
      for (const warning of result.warnings) toast.info(warning);
      closeDialog();
      await refresh();
    } catch (cause) {
      toast.error("清理失败（未写入）", { description: api.asError(cause) });
    } finally {
      setBusy(false);
    }
  }, [pending, refresh, closeDialog]);

  const restore = useCallback(
    async (id: string) => {
      setBusy(true);
      try {
        const result = await api.storageTrashRestore(id);
        if (result.failures.length > 0) {
          toast.warning(`恢复 ${result.restoredFiles} 个文件 / ${result.restoredRows} 行，${result.failures.length} 项失败`, {
            description: result.failures[0]?.error,
          });
        } else {
          toast.success(`已恢复 ${result.restoredFiles} 个文件 / ${result.restoredRows} 行`);
        }
        await refresh();
      } catch (cause) {
        toast.error("恢复失败", { description: api.asError(cause) });
      } finally {
        setBusy(false);
      }
    },
    [refresh],
  );

  const purge = useCallback(
    async (id: string) => {
      setBusy(true);
      try {
        const result = await api.storageTrashPurge(id);
        toast.success(`已彻底删除，释放 ${formatMb(result.bytes)}`);
        await refresh();
      } catch (cause) {
        toast.error("彻底删除失败", { description: api.asError(cause) });
      } finally {
        setBusy(false);
      }
    },
    [refresh],
  );

  const purgeAll = useCallback(async () => {
    setBusy(true);
    try {
      await api.storageTrashPurge();
      toast.success("回收站已清空");
      await refresh();
    } catch (cause) {
      toast.error("清空回收站失败", { description: api.asError(cause) });
    } finally {
      setBusy(false);
    }
  }, [refresh]);

  const reveal = useCallback(async (path: string, mode: "select" | "open") => {
    if (!path) {
      toast.error("没有可定位的路径");
      return;
    }
    try {
      await api.storageReveal(path, mode);
    } catch (cause) {
      toast.error("定位失败", { description: api.asError(cause) });
    }
  }, []);

  return {
    scan,
    sessions,
    trash,
    loading,
    busy,
    error,
    presence,
    lastRevalidatedAt,
    plan,
    dialogOpen,
    refresh,
    previewClean,
    closeDialog,
    confirmClean,
    restore,
    purge,
    purgeAll,
    reveal,
  };
}

function formatMb(bytes: number): string {
  if (bytes >= 1024 * 1024 * 1024) return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`;
  if (bytes >= 1024 * 1024) return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${bytes} B`;
}

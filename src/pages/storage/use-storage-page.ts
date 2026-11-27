import { useCallback, useEffect, useRef, useState } from "react";
import { toast } from "sonner";

import * as api from "@/lib/api";
import {
  invalidateStorageCache,
  isStorageScanCacheFresh,
  loadStorageScan,
  loadStorageSessions,
  loadStorageTrash,
  readStorageScanCache,
  readStorageSessionsCache,
  readStorageTrashCache,
} from "@/lib/storage-cache";
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
/** 总览渲染后的空闲预取延迟（第三轮第 6 条 6.4：1–2 秒）。 */
const PREFETCH_DELAY_MS = 1200;
/** 受保护项后台补算的轮询间隔与上限（拿到完整总量就停）。 */
const PENDING_POLL_MS = 2500;
const PENDING_POLL_LIMIT = 15;

/** 页签：决定「按需加载」的时机。 */
export type StorageTab = "categories" | "sessions" | "trash";

interface UseStoragePage {
  scan: StorageScan | null;
  sessions: StorageSession[] | null;
  /** 一键清理冗余副本会删掉的条数（服务端算，与 `redundantAll` 同源）。 */
  redundantCleanable: number;
  trash: StorageTrashPayload | null;
  /** 总览（首屏）加载中。 */
  scanLoading: boolean;
  /** 会话资产加载中（tab 内小 loader，不阻塞整页）。 */
  sessionsLoading: boolean;
  /** 回收站加载中。 */
  trashLoading: boolean;
  /** 总览错误：整页提示条。 */
  error: string | null;
  /** 会话资产错误：tab 内提示 + 重试。 */
  sessionsError: string | null;
  /** 回收站错误：tab 内提示 + 重试。 */
  trashError: string | null;
  busy: boolean;
  presence: Record<string, string>;
  lastRevalidatedAt: number | null;
  plan: StorageCleanPlan | null;
  dialogOpen: boolean;
  /** 「重新扫描」：只刷新已加载的 scope。 */
  refresh: () => Promise<void>;
  /** 页签切换：按需加载尚未拉过的数据。 */
  selectTab: (tab: StorageTab) => void;
  retrySessions: () => Promise<void>;
  retryTrash: () => Promise<void>;
  previewClean: (targets: StorageCleanTarget[]) => Promise<void>;
  closeDialog: () => void;
  confirmClean: () => Promise<void>;
  restore: (id: string) => Promise<void>;
  purge: (id: string) => Promise<void>;
  purgeAll: () => Promise<void>;
  reveal: (path: string, mode: "select" | "open", scope?: "storage" | "cwd") => Promise<void>;
}

/**
 * 存储管理页的数据编排（第三轮第 6 条 6.3）。
 *
 * 加载策略：
 * - 挂载只拉**总览**（首帧用客户端缓存，超过 60 秒后台刷新）；
 * - 会话资产 / 回收站切到对应页签才拉（tab 内 loader），总览渲染后空闲 1.2 秒预取；
 * - 清理 / 恢复 / 清空后作废客户端缓存并刷新当前 scope；
 * - 外部删除同步：focus / visibility 变化防抖 3 秒，只校验分类 + 前 80 条会话。
 */
export function useStoragePage(): UseStoragePage {
  // 首帧直接用缓存（stale-while-revalidate）：有缓存就不显示整页骨架。
  const [scan, setScan] = useState<StorageScan | null>(() => readStorageScanCache());
  const [sessions, setSessions] = useState<StorageSession[] | null>(() => {
    const cached = readStorageSessionsCache();
    return cached === null ? null : cached.sessions;
  });
  const [redundantCleanable, setRedundantCleanable] = useState(
    () => readStorageSessionsCache()?.redundantCleanable ?? 0,
  );
  const [trash, setTrash] = useState<StorageTrashPayload | null>(() => readStorageTrashCache());
  const [scanLoading, setScanLoading] = useState(true);
  const [sessionsLoading, setSessionsLoading] = useState(false);
  const [trashLoading, setTrashLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [sessionsError, setSessionsError] = useState<string | null>(null);
  const [trashError, setTrashError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [presence, setPresence] = useState<Record<string, string>>({});
  const [lastRevalidatedAt, setLastRevalidatedAt] = useState<number | null>(null);
  const [plan, setPlan] = useState<StorageCleanPlan | null>(null);
  const [pending, setPending] = useState<StorageCleanTarget[] | null>(null);
  const [dialogOpen, setDialogOpen] = useState(false);
  const sessionsRef = useRef<StorageSession[]>([]);
  sessionsRef.current = sessions ?? [];
  /** 当前页签，供「重新扫描」决定刷新哪些 scope。 */
  const tabRef = useRef<StorageTab>("categories");
  /** 已加载过的 scope：决定「重新扫描」要不要顺便拉会话 / 回收站。 */
  const loadedRef = useRef({ sessions: false, trash: false });

  const loadScan = useCallback(async (force: boolean) => {
    setScanLoading(true);
    setError(null);
    try {
      const nextScan = await loadStorageScan({ force });
      setScan(nextScan);
    } catch (cause) {
      setError(api.asError(cause));
    } finally {
      setScanLoading(false);
    }
  }, []);

  const loadSessions = useCallback(async (force: boolean) => {
    setSessionsLoading(true);
    setSessionsError(null);
    try {
      const payload = await loadStorageSessions({ force });
      setSessions(payload.sessions);
      setRedundantCleanable(payload.redundantCleanable ?? 0);
      loadedRef.current.sessions = true;
    } catch (cause) {
      setSessionsError(api.asError(cause));
    } finally {
      setSessionsLoading(false);
    }
  }, []);

  const loadTrash = useCallback(async (force: boolean) => {
    setTrashLoading(true);
    setTrashError(null);
    try {
      const payload = await loadStorageTrash({ force });
      setTrash(payload);
      loadedRef.current.trash = true;
    } catch (cause) {
      setTrashError(api.asError(cause));
    } finally {
      setTrashLoading(false);
    }
  }, []);

  /** 「重新扫描」：强制刷新当前已加载的 scope（未加载的不主动拉，避免又把整棵树跑一遍）。 */
  const refresh = useCallback(async () => {
    const tasks: Promise<void>[] = [loadScan(true)];
    if (loadedRef.current.sessions || tabRef.current === "sessions") {
      tasks.push(loadSessions(true));
    }
    if (loadedRef.current.trash || tabRef.current === "trash") {
      tasks.push(loadTrash(true));
    }
    await Promise.all(tasks);
  }, [loadScan, loadSessions, loadTrash]);

  const selectTab = useCallback(
    (tab: StorageTab) => {
      tabRef.current = tab;
      if (tab === "sessions" && sessionsRef.current.length === 0 && !loadedRef.current.sessions) {
        void loadSessions(false);
      }
      if (tab === "trash" && !loadedRef.current.trash) {
        void loadTrash(false);
      }
    },
    [loadSessions, loadTrash],
  );

  // 挂载：只有总览必须等。缓存还新鲜时也不跳过 —— 首帧已经用缓存渲染，
  // 这里只做 60 秒 TTL 内的「不重复请求」判断。
  useEffect(() => {
    const cached = readStorageScanCache();
    if (cached !== null && isStorageScanCacheFresh()) {
      setScan(cached);
      setScanLoading(false);
      return;
    }
    void loadScan(false);
  }, [loadScan]);

  // 总览就绪后空闲预取会话资产与回收站：切页签时基本是「即时」的。
  // 演示模式跳过（没有真实后端）；卸载时取消定时器。
  useEffect(() => {
    if (scan === null || api.isDemoMode()) return;
    const timer = setTimeout(() => {
      if (!loadedRef.current.sessions && sessionsRef.current.length === 0) {
        void loadSessions(false);
      }
      if (!loadedRef.current.trash) {
        void loadTrash(false);
      }
    }, PREFETCH_DELAY_MS);
    return () => {
      clearTimeout(timer);
    };
  }, [scan, loadSessions, loadTrash]);

  // 受保护项还在后台计量：轻量轮询到完整值（最多 15 次，不做无限轮询）。
  useEffect(() => {
    if (scan?.protectedPending !== true) return;
    let attempts = 0;
    let cancelled = false;
    const tick = window.setInterval(() => {
      attempts += 1;
      if (cancelled || attempts > PENDING_POLL_LIMIT) {
        window.clearInterval(tick);
        return;
      }
      // 只刷总览，且走缓存层（服务端在受保护项补齐后会重建快照）。
      void loadStorageScan()
        .then((next) => {
          if (cancelled) return;
          setScan(next);
          if (next.protectedPending !== true) window.clearInterval(tick);
        })
        .catch(() => {
          window.clearInterval(tick);
        });
    }, PENDING_POLL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(tick);
    };
  }, [scan?.protectedPending]);

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

  /** 破坏性操作成功后：客户端缓存全丢，服务端快照已被 core 作废。 */
  const refreshAfterWrite = useCallback(async () => {
    invalidateStorageCache("all");
    loadedRef.current = { sessions: true, trash: true };
    await Promise.all([loadScan(true), loadSessions(true), loadTrash(true)]);
  }, [loadScan, loadSessions, loadTrash]);

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
        const detail = `${formatMb(result.bytes)} 已移入回收站 ${
          result.deletedRows > 0 ? `· 删除 ${result.deletedRows} 条会话行` : ""
        }`;
        toast.success(`已清理 ${result.moved} 项`, {
          // 一键清理冗余副本会带一句「删了几条、按什么规则保留」的说明，优先展示它。
          description: result.note ? `${result.note} · ${detail}` : detail,
        });
      }
      for (const warning of result.warnings) toast.info(warning);
      closeDialog();
      await refreshAfterWrite();
    } catch (cause) {
      toast.error("清理失败（未写入）", { description: api.asError(cause) });
    } finally {
      setBusy(false);
    }
  }, [pending, refreshAfterWrite, closeDialog]);

  const restore = useCallback(
    async (id: string) => {
      setBusy(true);
      try {
        const result = await api.storageTrashRestore(id);
        if (result.failures.length > 0) {
          toast.warning(
            `恢复 ${result.restoredFiles} 个文件 / ${result.restoredRows} 行，${result.failures.length} 项失败`,
            { description: result.failures[0]?.error },
          );
        } else {
          toast.success(`已恢复 ${result.restoredFiles} 个文件 / ${result.restoredRows} 行`);
        }
        await refreshAfterWrite();
      } catch (cause) {
        toast.error("恢复失败", { description: api.asError(cause) });
      } finally {
        setBusy(false);
      }
    },
    [refreshAfterWrite],
  );

  const purge = useCallback(
    async (id: string) => {
      setBusy(true);
      try {
        const result = await api.storageTrashPurge(id);
        toast.success(`已彻底删除，释放 ${formatMb(result.bytes)}`);
        await refreshAfterWrite();
      } catch (cause) {
        toast.error("彻底删除失败", { description: api.asError(cause) });
      } finally {
        setBusy(false);
      }
    },
    [refreshAfterWrite],
  );

  const purgeAll = useCallback(async () => {
    setBusy(true);
    try {
      await api.storageTrashPurge();
      toast.success("回收站已清空");
      await refreshAfterWrite();
    } catch (cause) {
      toast.error("清空回收站失败", { description: api.asError(cause) });
    } finally {
      setBusy(false);
    }
  }, [refreshAfterWrite]);

  /**
   * 定位到文件管理器。
   *
   * `scope: "cwd"` 用于会话「工作目录」：只读打开任意绝对目录（后端拒绝 UNC / 设备 /
   * 相对路径，且不执行目标文件）。`fellBack` 为真时提示「已打开最近的上层目录」。
   */
  const reveal = useCallback(
    async (path: string, mode: "select" | "open", scope: "storage" | "cwd" = "storage") => {
      if (!path) {
        toast.error("没有可定位的路径");
        return;
      }
      try {
        const result = await api.storageReveal(path, mode, scope);
        if (result.fellBack) {
          toast.info("目录不存在，已打开最近的上层目录", {
            description: result.path,
          });
        }
      } catch (cause) {
        toast.error("定位失败", { description: api.asError(cause) });
      }
    },
    [],
  );

  return {
    scan,
    sessions,
    redundantCleanable,
    trash,
    scanLoading,
    sessionsLoading,
    trashLoading,
    error,
    sessionsError,
    trashError,
    busy,
    presence,
    lastRevalidatedAt,
    plan,
    dialogOpen,
    refresh,
    selectTab,
    retrySessions: () => loadSessions(true),
    retryTrash: () => loadTrash(true),
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

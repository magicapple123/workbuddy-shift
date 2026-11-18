import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import * as api from "@/lib/api";
import type { SessionGroupClient, SessionGroupSummary, WbVariant } from "@/lib/types";

import { PAGE_SIZE, groupListCache } from "../constants";

export interface SessionGroupsController {
  groups: SessionGroupSummary[];
  /** 被列表隐藏的失效组数量：> 0 时仍要显示「清理失效关联组」入口。 */
  deadGroupCount: number;
  /**
   * 清理被阻塞的原因（`null` = 可清理）。
   *
   * 后端给的「点了必失败」的原因：入口要禁用并展示它，`requestPrune` 也要在发请求前拦下。
   */
  pruneBlocked: string | null;
  storeStatus: "missing" | "ready" | "unavailable";
  storeError: string | null;
  loading: boolean;
  loadError: string | null;
  statusFilter: string;
  setStatusFilter: (value: string) => void;
  sortOrder: string;
  setSortOrder: (value: string) => void;
  page: number;
  setPage: (value: number) => void;
  filteredGroups: SessionGroupSummary[];
  pageCount: number;
  currentPage: number;
  visibleGroups: SessionGroupSummary[];
  reloadGroups: () => Promise<void>;
}

/**
 * 关联会话列表：加载、缓存快照、状态筛选、排序与分页。
 *
 * 竞态守卫沿用原来的两道：listRequestId 自增 + 当前 client:scope 上下文键，
 * 切换客户端/档位后旧请求的结果直接丢弃，避免把上一个客户端的数据写入当前列表。
 */
export function useSessionGroups(
  client: SessionGroupClient,
  scope: WbVariant | undefined,
): SessionGroupsController {
  const [groups, setGroups] = useState<SessionGroupSummary[]>([]);
  const [deadGroupCount, setDeadGroupCount] = useState(0);
  const [pruneBlocked, setPruneBlocked] = useState<string | null>(null);
  const [storeStatus, setStoreStatus] = useState<"missing" | "ready" | "unavailable">("missing");
  const [storeError, setStoreError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [statusFilter, setStatusFilter] = useState("all");
  const [sortOrder, setSortOrder] = useState("recent");
  const [page, setPage] = useState(1);

  const listRequestId = useRef(0);
  const contextRef = useRef({ client, scope });
  contextRef.current = { client, scope };
  const listContextKey = `${client}:${scope ?? "all"}`;
  const currentContextKey = () => `${contextRef.current.client}:${contextRef.current.scope ?? "all"}`;

  const reloadGroups = useCallback(async () => {
    const key = `${client}:${scope ?? "all"}`;
    const requestId = ++listRequestId.current;
    // 已有快照时不再退回骨架屏：刷新是「替换数据」，不是「重新加载页面」。
    if (!groupListCache.has(key)) setLoading(true);
    setLoadError(null);
    setStoreError(null);
    try {
      const result = await api.listSessionGroups(client, scope);
      if (currentContextKey() !== key || listRequestId.current !== requestId) return;
      groupListCache.set(key, {
        groups: result.groups,
        storeStatus: result.storeStatus,
        storeError: result.storeError ?? null,
        deadGroupCount: result.deadGroupCount ?? 0,
        pruneBlocked: result.pruneBlocked ?? null,
      });
      setGroups(result.groups);
      setDeadGroupCount(result.deadGroupCount ?? 0);
      setPruneBlocked(result.pruneBlocked ?? null);
      setStoreStatus(result.storeStatus);
      setStoreError(result.storeError ?? null);
    } catch (error) {
      if (currentContextKey() !== key || listRequestId.current !== requestId) return;
      setGroups([]);
      setLoadError(api.asError(error));
      groupListCache.delete(key);
    } finally {
      if (currentContextKey() === key && listRequestId.current === requestId) setLoading(false);
    }
  }, [client, scope]);

  useEffect(() => {
    let live = true;
    const requestId = ++listRequestId.current;
    const key = listContextKey;
    const cached = groupListCache.get(key);
    setLoading(!cached);
    setLoadError(null);
    if (cached) {
      setGroups(cached.groups);
      setDeadGroupCount(cached.deadGroupCount);
      setPruneBlocked(cached.pruneBlocked);
      setStoreStatus(cached.storeStatus);
      setStoreError(cached.storeError);
    }
    api.listSessionGroups(client, scope).then((result) => {
      if (!live || listRequestId.current !== requestId || currentContextKey() !== key) return;
      groupListCache.set(key, {
        groups: result.groups,
        storeStatus: result.storeStatus,
        storeError: result.storeError ?? null,
        deadGroupCount: result.deadGroupCount ?? 0,
        pruneBlocked: result.pruneBlocked ?? null,
      });
      setGroups(result.groups);
      setDeadGroupCount(result.deadGroupCount ?? 0);
      setPruneBlocked(result.pruneBlocked ?? null);
      setStoreStatus(result.storeStatus);
      setStoreError(result.storeError ?? null);
    }).catch((error) => {
      if (!live || listRequestId.current !== requestId || currentContextKey() !== key) return;
      // 有旧快照时保留画面，把失败留给下一次刷新/操作提示，不把已有内容清空。
      if (cached) return;
      setGroups([]);
      setLoadError(api.asError(error));
    }).finally(() => {
      if (live && listRequestId.current === requestId && currentContextKey() === key) setLoading(false);
    });
    return () => { live = false; };
  }, [client, scope, listContextKey]);

  const filteredGroups = useMemo(() => {
    const result = groups.filter((group) => {
      return statusFilter === "all" || group.summaryStatus === statusFilter;
    });
    result.sort((left, right) => sortOrder === "oldest"
      ? left.latestActivityAt - right.latestActivityAt
      : left.latestActivityAt === right.latestActivityAt
        ? left.title.localeCompare(right.title)
        : right.latestActivityAt - left.latestActivityAt);
    return result;
  }, [groups, statusFilter, sortOrder]);
  const pageCount = Math.max(1, Math.ceil(filteredGroups.length / PAGE_SIZE));
  const currentPage = Math.min(page, pageCount);
  const visibleGroups = filteredGroups.slice((currentPage - 1) * PAGE_SIZE, currentPage * PAGE_SIZE);

  useEffect(() => { setPage(1); }, [statusFilter, sortOrder, client, scope]);

  return {
    groups,
    deadGroupCount,
    pruneBlocked,
    storeStatus,
    storeError,
    loading,
    loadError,
    statusFilter,
    setStatusFilter,
    sortOrder,
    setSortOrder,
    page,
    setPage,
    filteredGroups,
    pageCount,
    currentPage,
    visibleGroups,
    reloadGroups,
  };
}

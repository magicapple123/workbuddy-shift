import { useEffect, useRef, useState } from "react";

import * as api from "@/lib/api";
import type {
  SessionGroupClient,
  SessionGroupCurrentAccount,
  SessionGroupDetail,
  SessionGroupMemberDetail,
  SessionGroupUnifyPlan,
  WbVariant,
} from "@/lib/types";

export interface SessionDetailController {
  detail: SessionGroupDetail | null;
  detailLoading: boolean;
  detailError: string | null;
  sourceMemberId: string;
  setSourceMemberId: (id: string) => void;
  addTargetId: string;
  setAddTargetId: (id: string) => void;
  currentAccounts: SessionGroupCurrentAccount[];
  unifyPlan: SessionGroupUnifyPlan | null;
  unifyLoading: string | null;
  setUnifyPlan: (plan: SessionGroupUnifyPlan | null) => void;
  setDetailError: (error: string | null) => void;
  /** 打开详情前复位：清空旧内容、进入加载态并作废在途预览请求（与原来 openDetail 同步完成）。 */
  resetForOpen: () => void;
  /** 预检各目标副本并生成统一预览方案；busy 为调用方当前的动作忙状态。 */
  prepareUnify: (source: SessionGroupMemberDetail, busy: boolean) => Promise<void>;
  /** 重新拉取当前选中组的详情与列表（动作成功后的统一刷新）。 */
  reloadSelectedGroup: () => Promise<void>;
}

interface Options {
  client: SessionGroupClient;
  scope: WbVariant | undefined;
  selectedGroupId: string | null;
  detailOpen: boolean;
  reloadGroups: () => Promise<void>;
}

const detailKeyOf = (context: {
  client: SessionGroupClient;
  scope: WbVariant | undefined;
  selectedGroupId: string | null;
}): string => `${context.client}:${context.scope ?? "all"}:${context.selectedGroupId ?? "none"}`;

/**
 * 关联会话详情：详情加载、当前登录账号探测与统一预览方案。
 *
 * 竞态守卫：detailRequestId / previewRequestId 自增 + 当前
 * client:scope:selectedGroupId 上下文键；弹窗关闭或切换会话组后旧响应一律丢弃。
 */
export function useSessionDetail({
  client,
  scope,
  selectedGroupId,
  detailOpen,
  reloadGroups,
}: Options): SessionDetailController {
  const [detail, setDetail] = useState<SessionGroupDetail | null>(null);
  const [detailLoading, setDetailLoading] = useState(false);
  const [detailError, setDetailError] = useState<string | null>(null);
  const [sourceMemberId, setSourceMemberId] = useState("");
  const [addTargetId, setAddTargetId] = useState("");
  const [unifyPlan, setUnifyPlan] = useState<SessionGroupUnifyPlan | null>(null);
  const [unifyLoading, setUnifyLoading] = useState<string | null>(null);
  const [currentAccounts, setCurrentAccounts] = useState<SessionGroupCurrentAccount[]>([]);

  const detailRequestId = useRef(0);
  const previewRequestId = useRef(0);
  const contextRef = useRef({ client, scope, selectedGroupId });
  contextRef.current = { client, scope, selectedGroupId };
  const detailContextKey = detailKeyOf({ client, scope, selectedGroupId });

  useEffect(() => {
    if (!selectedGroupId) { setCurrentAccounts([]); return; }
    let live = true;
    setCurrentAccounts([]);
    const reads = client === "workbuddy"
      ? [api.getStatus("cn").then((status) => ({ variant: "cn" as const, uid: status.current?.uid, running: status.running })),
        api.getStatus("ai").then((status) => ({ variant: "ai" as const, uid: status.current?.uid, running: status.running }))]
      : client === "codebuddyIde"
        ? [api.getCodebuddyCnIdeStatus().then((status) => ({ variant: "cn" as const, accountId: status.activeAccountId })),
          api.getCodebuddyIdeStatus().then((status) => ({ variant: "ai" as const, accountId: status.activeAccountId }))]
        // JetBrains 必须读自己的状态：先前掉进下面的 VS Code 分支，详情里的「当前账号」
        // 标记取的是 VS Code 的登录账号，换个 IDE 登录就标错人。
        : client === "jetbrains"
          ? [api.getJetbrainsStatus().then((status) => ({ accountId: status.activeAccountId, running: status.running }))]
          : [api.getVscodeExtStatus().then((status) => ({ accountId: status.activeAccountId, running: status.running }))];
    void Promise.allSettled(reads).then((results) => {
      if (live) setCurrentAccounts(results.flatMap((result) => result.status === "fulfilled" ? [result.value] : []));
    });
    return () => { live = false; };
  }, [client, selectedGroupId, detail]);

  function resetForOpen() {
    previewRequestId.current += 1;
    setDetail(null);
    setDetailError(null);
    setDetailLoading(true);
    setUnifyPlan(null);
    setUnifyLoading(null);
  }

  useEffect(() => {
    // 只有弹窗打开且已选中会话组才需要详情：关闭时同样进入这里，作废在途请求并停掉加载态，
    // 但保留已渲染内容给退场动画（重开时加载流程会清空）。
    if (!detailOpen || !selectedGroupId) {
      detailRequestId.current += 1;
      previewRequestId.current += 1;
      setDetailLoading(false);
      setUnifyPlan(null);
      setUnifyLoading(null);
      if (!selectedGroupId) {
        setDetail(null);
        setDetailError(null);
        setSourceMemberId("");
        setAddTargetId("");
      }
      return;
    }
    let live = true;
    const requestId = ++detailRequestId.current;
    const key = detailContextKey;
    setDetail(null);
    setDetailError(null);
    setDetailLoading(true);
    previewRequestId.current += 1;
    setUnifyPlan(null);
    setUnifyLoading(null);
    api.getSessionGroup(client, selectedGroupId, scope).then((result) => {
      if (!live || detailRequestId.current !== requestId || detailKeyOf(contextRef.current) !== key) return;
      setDetail(result);
      setSourceMemberId(result.safeSourceMemberId ?? "");
      setAddTargetId(result.addTargets[0]?.id ?? "");
    }).catch((error) => {
      if (!live || detailRequestId.current !== requestId || detailKeyOf(contextRef.current) !== key) return;
      setDetailError(api.asError(error));
    }).finally(() => {
      if (live && detailRequestId.current === requestId && detailKeyOf(contextRef.current) === key) setDetailLoading(false);
    });
    return () => { live = false; };
  }, [client, scope, selectedGroupId, detailContextKey, detailOpen]);

  async function prepareUnify(source: SessionGroupMemberDetail, busy: boolean) {
    if (!detail || !source.canBeSource || busy || unifyLoading) return;
    const requestId = ++previewRequestId.current;
    const key = detailContextKey;
    const groupId = detail.groupId;
    const targets = detail.members.filter((member) => member.linkState === "active" && member.memberId !== source.memberId);
    setUnifyPlan(null);
    setUnifyLoading(source.memberId);
    const results = await Promise.allSettled(targets.map((target) => api.previewSessionGroupPair({
      client, groupId, sourceMemberId: source.memberId, targetMemberId: target.memberId, variantScope: scope,
    })));
    if (previewRequestId.current !== requestId || detailKeyOf(contextRef.current) !== key) return;
    setUnifyLoading(null);
    setUnifyPlan({
      client, groupId, sourceMemberId: source.memberId, sourceName: source.accountName,
      targets: targets.map((target, index) => {
        const result = results[index];
        return { memberId: target.memberId, accountName: target.accountName,
          preview: result.status === "fulfilled" ? result.value : null,
          error: result.status === "rejected" ? api.asError(result.reason) : null };
      }),
    });
  }

  async function reloadSelectedGroup() {
    const key = detailKeyOf({ client, scope, selectedGroupId });
    // An operation may finish after the user selected another group/client.
    if (detailKeyOf(contextRef.current) !== key || !selectedGroupId) return;
    const requestId = ++detailRequestId.current;
    setDetailLoading(true);
    setDetailError(null);
    previewRequestId.current += 1;
    setUnifyPlan(null);
    setUnifyLoading(null);
    await reloadGroups();
    if (detailKeyOf(contextRef.current) !== key) return;
    if (!selectedGroupId) return;
    try {
      const next = await api.getSessionGroup(client, selectedGroupId, scope);
      if (detailKeyOf(contextRef.current) !== key || detailRequestId.current !== requestId) return;
      setDetail(next);
      setSourceMemberId(next.safeSourceMemberId ?? sourceMemberId);
      setAddTargetId(next.addTargets[0]?.id ?? "");
      previewRequestId.current += 1;
      setUnifyPlan(null);
      setUnifyLoading(null);
    } catch (error) {
      if (detailKeyOf(contextRef.current) === key && detailRequestId.current === requestId) setDetailError(api.asError(error));
    } finally {
      if (detailRequestId.current === requestId) setDetailLoading(false);
    }
  }

  return {
    detail,
    detailLoading,
    detailError,
    sourceMemberId,
    setSourceMemberId,
    addTargetId,
    setAddTargetId,
    currentAccounts,
    unifyPlan,
    unifyLoading,
    setUnifyPlan,
    setDetailError,
    resetForOpen,
    prepareUnify,
    reloadSelectedGroup,
  };
}

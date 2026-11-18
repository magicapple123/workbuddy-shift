import type {
  AccountMeta,
  Session,
  SessionLinkPreviewGroup,
  SessionLinksPreview,
  SessionGroupClient,
  SessionGroupDetail,
  SessionGroupList,
  SessionGroupPairPreview,
  SessionGroupSummary,
  SessionMemberVersionStatus,
  SessionSyncVerdict,
  TokenStatistics,
  TokenStatsGroup,
  TokenStatsRequestRow,
  TokenStatsSource,
  TokenStatsTotals,
  VscodeSessionList,
} from "../types";
import { accountVariant } from "../variant";
import { atLocalTime, intlAccountA, localDate } from "./accounts";
import type { DemoContext } from "./accounts";

/**
 * 会话演示数据：会话组、IDE 会话列表、跨端复制预览与 Token 统计请求明细。
 *
 * 会话组 fixture 按客户端/档位生成；ID 统计用同一份账号列表做档位区分。
 */

export const SESSION_DEMO_COMMANDS = [
  "list_account_sessions",
  "list_session_groups",
  "get_session_group",
  "preview_session_group_pair",
  "list_vscode_sessions",
  "list_codebuddy_ide_sessions",
  "list_codebuddy_intl_ide_sessions",
  "session_links_preview_cross",
  "codebuddy_ide_session_links_preview",
  "codebuddy_intl_ide_session_links_preview",
  "vscode_session_links_preview",
  "get_token_statistics",
] as const;

function demoTokenTotals(input: number, output: number, cacheRead: number, cacheWrite: number, records: number): TokenStatsTotals {
  return { total: input + output + cacheWrite, input, output, cacheRead, cacheWrite, uncachedInput: Math.max(0, input - cacheRead), records, cacheHitRate: input > 0 ? cacheRead / input : null };
}

function demoTokenGroup(key: string, input: number, output: number, cacheRead: number, cacheWrite: number, records: number): TokenStatsGroup {
  return { key, ...demoTokenTotals(input, output, cacheRead, cacheWrite, records) };
}

function demoTokenSession(key: string, title: string, project: string, input: number, output: number, cacheRead: number, cacheWrite: number, records: number): TokenStatsGroup {
  const keyParts = key.split(" · ");
  return { ...demoTokenGroup(key, input, output, cacheRead, cacheWrite, records), title, project, sessionId: keyParts[keyParts.length - 1] };
}

/** 演示用请求明细：约 120 条，时间戳落在最近 14 天内且按时间倒序。 */
function demoTokenRequests(scale: number): TokenStatsRequestRow[] {
  const models = ["deepseek-v4-flash", "deepseek-v4-flash", "kimi-k3-1", "deepseek-v4-flash", "glm-5.2"];
  const sessions = [
    { project: "workbuddy-shift-desktop", sessionId: "token-stats-dashboard", title: "完善 Token 统计仪表盘与本地用量分析" },
    { project: "workbuddy-shift-desktop", sessionId: "account-card-redesign", title: "统一账号卡片视觉和交互" },
    { project: "my-code-teams", sessionId: "settings-agent-acp", title: "设计 Agent 与 ACP 管理设置" },
    { project: "LetterTotTown", sessionId: "character-audio", title: "补全角色成语双音频" },
  ];
  // 每天 9 条、小时降序，保证整体严格倒序（最新在前）；从昨天开始，
  // 避免演示时间戳落在当前时刻之后。
  const hours = [23, 21, 20, 17, 16, 15, 14, 11, 10];

  return Array.from({ length: 120 }, (_, index) => {
    const session = sessions[Math.floor(index / 3) % sessions.length];
    const factor = 0.55 + ((index * 37) % 23) / 20;
    const input = Math.round(310_000 * factor * scale);
    const cacheRead = Math.round(input * 0.87);
    const output = Math.round(20_000 * factor * scale);
    const cacheWrite = index % 4 === 0 ? 0 : Math.round(4_000 * factor * scale);
    // 思考过程是 output 的子集：比例随行变化，保证 thinking <= output。
    const thinking = Math.round(output * [0.34, 0.12, 0.05, 0.41, 0.22][index % 5]);
    return {
      timestamp: atLocalTime(Math.floor(index / 9) + 1, hours[index % 9], (index * 13) % 60),
      model: models[index % models.length],
      project: session.project,
      sessionId: session.sessionId,
      title: session.title,
      input,
      output,
      cacheRead,
      cacheWrite,
      // 与后端明细行同口径：input 已含 cacheRead，未命中部分为两者之差。
      uncachedInput: Math.max(0, input - cacheRead),
      thinking,
      total: input + output + cacheWrite,
    };
  });
}

function demoTokenSource(source: TokenStatsSource["source"], scale: number): TokenStatsSource {
  const daily = Array.from({ length: 14 }, (_, index) => {
    const wave = [0.62, 0.86, 1.1, 0.72, 1.3, 0.94, 0.38][index % 7] * scale;
    return demoTokenGroup(localDate(13 - index), Math.round(7_600_000 * wave), Math.round(480_000 * wave), Math.round(6_650_000 * wave), Math.round(95_000 * wave), Math.round(24 * wave));
  });
  const summary = daily.reduce((sum, row) => demoTokenTotals(sum.input + row.input, sum.output + row.output, sum.cacheRead + row.cacheRead, sum.cacheWrite + row.cacheWrite, sum.records + row.records), demoTokenTotals(0, 0, 0, 0, 0));
  const hours = Array.from({ length: 7 * 24 }, (_, index) => {
    const day = Math.floor(index / 24); const hour = index % 24;
    const active = Math.max(0.02, Math.exp(-Math.pow(hour - (day >= 5 ? 22 : 15), 2) / 22));
    return demoTokenGroup(`${day}-${hour}`, Math.round(720_000 * active * scale), Math.round(41_000 * active * scale), Math.round(610_000 * active * scale), 0, Math.max(1, Math.round(6 * active * scale)));
  });
  const projects = [
    demoTokenGroup("workbuddy-shift-desktop", 42_800_000 * scale, 2_400_000 * scale, 37_100_000 * scale, 420_000 * scale, Math.round(148 * scale)),
    demoTokenGroup("my-code-teams", 25_600_000 * scale, 1_650_000 * scale, 21_900_000 * scale, 260_000 * scale, Math.round(96 * scale)),
    demoTokenGroup("LetterTotTown", 11_900_000 * scale, 920_000 * scale, 9_700_000 * scale, 110_000 * scale, Math.round(51 * scale)),
  ];
  const models = [
    demoTokenGroup("deepseek-v4-flash", 56_400_000 * scale, 3_200_000 * scale, 49_100_000 * scale, 530_000 * scale, Math.round(210 * scale)),
    demoTokenGroup("kimi-k3-1", 17_300_000 * scale, 1_140_000 * scale, 14_200_000 * scale, 180_000 * scale, Math.round(61 * scale)),
    demoTokenGroup("glm-5.2", 6_600_000 * scale, 630_000 * scale, 5_400_000 * scale, 80_000 * scale, Math.round(24 * scale)),
  ];
  const sessions = [
    demoTokenSession("workbuddy-shift-desktop · token-stats-dashboard", "完善 Token 统计仪表盘与本地用量分析", "workbuddy-shift-desktop", 18_700_000 * scale, 1_050_000 * scale, 16_100_000 * scale, 160_000 * scale, Math.round(72 * scale)),
    demoTokenSession("my-code-teams · settings-agent-acp", "设计 Agent 与 ACP 管理设置", "my-code-teams", 13_200_000 * scale, 890_000 * scale, 11_300_000 * scale, 120_000 * scale, Math.round(55 * scale)),
    demoTokenSession("LetterTotTown · character-audio", "补全角色成语双音频", "LetterTotTown", 8_600_000 * scale, 640_000 * scale, 7_200_000 * scale, 80_000 * scale, Math.round(38 * scale)),
    demoTokenSession("workbuddy-shift-desktop · account-card-redesign", "统一账号卡片视觉和交互", "workbuddy-shift-desktop", 6_300_000 * scale, 410_000 * scale, 5_400_000 * scale, 50_000 * scale, Math.round(29 * scale)),
  ];
  const now = Date.now();
  return {
    source, summary, models, projects, sessions, daily, hours,
    filesScanned: source === "workbuddy" ? 63 : source === "codebuddy-ide" ? 17 : 41,
    parseErrors: 0,
    coverageStartAt: now - 13 * 86_400_000,
    coverageEndAt: now,
    // 只有 CodeBuddy CLI 来源返回请求明细，与真实后端行为一致。
    ...(source === "codebuddy-cli" ? { requests: demoTokenRequests(scale) } : {}),
  };
}

/** 国际版数据源：演示环境不构造数据，保持真实的「空集」形态（页面显示空状态）。 */
function emptyTokenSource(source: TokenStatsSource["source"]): TokenStatsSource {
  return {
    source,
    summary: demoTokenTotals(0, 0, 0, 0, 0),
    models: [],
    projects: [],
    sessions: [],
    daily: [],
    hours: [],
    filesScanned: 0,
    parseErrors: 0,
  };
}

function demoTokenStatistics(days?: number): TokenStatistics {
  return { generatedAt: Date.now(), rangeDays: days ?? null, sources: [demoTokenSource("workbuddy", 1), emptyTokenSource("workbuddy-ai"), demoTokenSource("codebuddy-cli", 0.58), demoTokenSource("codebuddy-ide", 0.36)] };
}

/** 演示用的 IDE 会话列表：国内版 / 国际版共用同一套存储，只有来源 uid 不同。 */
function demoIdeSessionList(sourceUid: string): VscodeSessionList {
  return {
    sourceUid,
    skipped: 0,
    dataRoot: "/demo/CodeBuddyExtension/Data",
    sessions: [
      { id: "4a598bb4e3144a799e602dd6c9091d03", workspaceHash: "6c4b8aec50b679a6fc4023b379160418", title: "两分钟后自动回复设置", updatedAt: Date.now() - 1000 * 60 * 8, type: "craft", hasHistory: true },
      { id: "1d2e3f405162738495a6b7c8d9e0f1a2", workspaceHash: "6c4b8aec50b679a6fc4023b379160418", title: "整理会话复制文案", updatedAt: Date.now() - 1000 * 60 * 90, type: "craft", hasHistory: true },
      { id: "a1b2c3d4e5f60718293a4b5c6d7e8f90", workspaceHash: "ffeeddccbbaa99887766554433221100", title: "工作区索引结构确认", updatedAt: Date.now() - 1000 * 60 * 60 * 30, type: "craft", hasHistory: true },
    ],
  };
}

/** 演示用的「会话管理页」会话列表：任务（playground）与空间（按 space_id 分组）两种形态。 */
function demoAccountSessions(intl: boolean): Session[] {
  const now = Date.now();
  const project = intl ? "/Users/demo/WorkBuddy AI/2026-09-18-01-33-29" : "/Users/demo/WorkBuddy/2026-09-18-01-33-29";
  const repo = intl ? "/Users/demo/projects/intl-app" : "/Users/demo/projects/workbuddy-shift";
  // 空间名来自 workbuddy.db 的 `spaces.name`（不是 cwd 末段）：演示数据按真实库形状补上
  // spaceId / spaceName，演示模式与真机看到同一套「空间 → 会话」两层结构。
  const appSpace = intl
    ? { spaceId: "demo-space-intl-app", spaceName: "intl-app" }
    : { spaceId: "demo-space-workbuddy-shift", spaceName: "workbuddy-shift" };
  const notesSpace = intl
    ? { spaceId: "demo-space-intl-notes", spaceName: "notes" }
    : { spaceId: "demo-space-notes", spaceName: "notes" };
  return [
    { id: "demo-sess-task-1", title: "整理会话复制文案", cwd: project, updatedAt: now - 1000 * 60 * 8, hasHistory: true, isPlayground: true, ...appSpace },
    { id: "demo-sess-task-2", title: "生成 changelog 草稿", cwd: project, updatedAt: now - 1000 * 60 * 95, hasHistory: true, isPlayground: true, ...appSpace },
    { id: "demo-sess-space-1", title: "跨档复制内核改造", cwd: repo, updatedAt: now - 1000 * 60 * 40, hasHistory: true, ...appSpace },
    { id: "demo-sess-space-2", title: "会话管理页骨架", cwd: repo, updatedAt: now - 1000 * 60 * 60 * 5, hasHistory: true, ...appSpace },
    { id: "demo-sess-space-3", title: "发布前回归检查", cwd: "/Users/demo/Documents/notes", updatedAt: now - 1000 * 60 * 60 * 26, hasHistory: true, ...notesSpace },
  ];
}

/** 会话管理页的副本状态演示项（判定口径与后端 `decide_sync` 一致）。 */
function demoPreviewGroup(
  sessionId: string,
  title: string,
  verdict: SessionSyncVerdict,
  extraA: number,
  extraB: number,
): SessionLinkPreviewGroup {
  return {
    groupId: `demo-group-${sessionId}`,
    title,
    cwd: "/Users/demo",
    verdict,
    extraA,
    extraB,
    common: 10,
    defaultChecked: verdict === "fastForward",
    availableModes: verdict === "fastForward" ? ["fastForward"] : verdict === "diverge" ? ["overwrite"] : [],
    reason: "演示数据",
    // 演示凭据：让「同步 / 覆盖目标」按钮在演示页可见（点击由 DemoAction 拦截）。
    previewToken: `demo-token-${sessionId}`,
    recordCount: { source: 10 + extraA, target: 10 + extraB, baseline: 10 },
    source: { memberId: `m-src-${sessionId}`, uid: "demo-source", accountId: null, sessionId, state: "active" },
    target: {
      memberId: `m-tgt-${sessionId}`,
      uid: "demo-target",
      accountId: null,
      sessionId: `${sessionId}-copy`,
      state: "active",
    },
  };
}

export function buildDemoSessionGroups(
  client: SessionGroupClient,
  scope: "cn" | "ai" | null,
  allAccounts: AccountMeta[],
): SessionGroupDetail[] {
  const candidates = allAccounts.filter((account) => {
    if (client === "codebuddyIde") return accountVariant(account) === scope;
    return true;
  });
  const memberAccounts = candidates.length > 0 ? candidates : allAccounts.slice(0, 2);
  const scenario = [
    { title: "跨档复制内核改造", status: "behind" as const, count: 2 },
    { title: "会话筛选和分页", status: "latest" as const, count: 3 },
    { title: "整理长时间未完成的任务", status: "diverge" as const, count: 2 },
    { title: "恢复后历史会话索引", status: "missing" as const, count: 2 },
  ];
  return scenario.map((item, groupIndex) => {
    const groupId = `demo-${client}-${scope ?? "all"}-${groupIndex + 1}`;
    const selectedAccounts = client === "workbuddy" && item.status === "diverge" && memberAccounts.length >= 5
      ? [memberAccounts[1], memberAccounts[2], memberAccounts[4], memberAccounts[0], memberAccounts[3]]
      : Array.from({ length: Math.min(item.count, memberAccounts.length) }, (_, index) => memberAccounts[index]);
    const sharedBaseScenario = client === "workbuddy" && item.status === "diverge" && selectedAccounts.length === 5;
    const members = selectedAccounts.map((account, memberIndex) => {
      let versionStatus: SessionMemberVersionStatus = "latest";
      let contentState: "ready" | "missing" | "unavailable" = "ready";
      let reason = "内容与操作来源一致";
      if (item.status === "behind" && memberIndex > 0) {
        versionStatus = "behind";
        reason = "目标账号没有独有改动，来源账号新增 3 条，可以安全同步";
      } else if (item.status === "diverge") {
        versionStatus = "diverge";
        reason = sharedBaseScenario
          ? memberIndex < 3 ? "与另外两个账号处于相同旧版；两条独立分支都包含此内容" : "从共同旧版新增了内容；与另一条分支的更新不同"
          : "双方都有更新；覆盖会替换目标账号的完整内容";
      } else if (item.status === "missing" && memberIndex > 0) {
        versionStatus = "missing";
        contentState = "missing";
        reason = "目标会话正文不存在，无法确认或同步";
      }
      return {
        memberId: `${groupId}-member-${memberIndex + 1}`,
        accountId: account.id,
        uid: account.uid ?? `demo-uid-${memberIndex}`,
        sessionId: `${groupId}-session-${memberIndex + 1}`,
        accountName: account.nickname || account.email || account.uid || "演示账号",
        variant: accountVariant(account),
        linkState: "active" as const,
        versionStatus,
        title: item.title,
        projectLabel: client === "workbuddy" ? "项目 workbuddy-shift" : "工作区 3c1f8a92",
        updatedAt: Date.now() - (sharedBaseScenario ? memberIndex < 3 ? 8 + memberIndex : memberIndex - 3 : memberIndex + groupIndex * 2) * 1000 * 60 * 37,
        recordCount: contentState === "ready" ? (sharedBaseScenario ? memberIndex < 3 ? 14 : memberIndex === 3 ? 17 : 19 : item.status === "latest" ? 14 : Math.max(4, 14 - memberIndex * 3)) : null,
        contentPreview: contentState === "ready" ? [
          { speaker: "用户", text: item.status === "diverge" ? "整理长期未完成的任务，并确认下一步安排。" : "继续处理这个会话。" },
          { speaker: "助手", text: sharedBaseScenario ? memberIndex < 3 ? "已整理共同旧版的待跟进事项。" : memberIndex === 3 ? "已补充国内版分支的处理记录。" : "已补充国际版分支的处理记录。" : item.status === "diverge" ? (memberIndex === 0 ? "已整理待跟进事项，建议先处理账号同步与会话列表。" : "已补充测试和发布前检查，建议先核对剩余问题。") : "已记录当前进度。" },
        ] : [],
        contentState,
        reason,
        canBeSource: contentState === "ready",
      };
    });
    const summary: SessionGroupSummary = {
      key: `${client}:${scope ?? "all"}:${groupId}`,
      client,
      variantScope: scope,
      groupId,
      groupVariant: scope ?? members[0].variant,
      title: item.title,
      projectLabel: members[0].projectLabel,
      latestActivityAt: Math.max(...members.map((member) => member.updatedAt)),
      memberCount: members.length,
      activeMemberCount: members.length,
      accountNames: members.map((member) => member.accountName),
      summaryStatus: item.status,
      summaryText: item.status === "behind" ? "有副本落后，可安全同步" : item.status === "latest" ? "关联副本内容一致" : sharedBaseScenario ? "3 个账号在共同旧版，另有 2 条独立更新" : item.status === "diverge" ? "多个副本有不同更新，需要选择来源" : "有副本内容缺失",
      safeSourceMemberId: item.status === "behind" || item.status === "latest" ? members[0].memberId : null,
      hasSafeSource: item.status === "behind" || item.status === "latest",
    };
    const linkedAccountIds = new Set(members.map((member) => member.accountId));
    return {
      ...summary,
      members,
      addTargets: candidates.filter((account) => !linkedAccountIds.has(account.id)),
      ...(sharedBaseScenario ? { divergence: { commonMemberIds: members.slice(0, 3).map((member) => member.memberId), branches: members.slice(3).map((member) => [member.memberId]) } } : {}),
    };
  });
}

export function sessionsDemoResponse(
  command: string,
  ctx: DemoContext,
  args?: Record<string, unknown>,
): unknown | undefined {
  const { demoAccounts, groupClient, groupScope, demoGroups } = ctx;
  switch (command) {
    // 会话管理页：按账号返回演示会话；国际版账号给一套不同 cwd，混排时可见档位差异。
    case "list_account_sessions": {
      const account = demoAccounts.find((item) => item.id === args?.accountId) ?? demoAccounts[0];
      return {
        sessions: demoAccountSessions(account.id === intlAccountA.id),
        current: account.uid ?? "demo-uid",
        variant: accountVariant(account),
      };
    }

    case "list_session_groups": {
      if (!groupClient || (groupClient === "codebuddyIde" && !groupScope) || (groupClient !== "codebuddyIde" && groupScope)) {
        throw new Error("会话客户端或档位参数无效");
      }
      const groups = demoGroups.map(({ members: _members, addTargets: _targets, divergence: _divergence, ...summary }) => summary);
      return { client: groupClient, variantScope: groupScope, storeStatus: "ready", groups } satisfies SessionGroupList;
    }
    case "get_session_group": {
      if (!groupClient || (groupClient === "codebuddyIde" && !groupScope) || (groupClient !== "codebuddyIde" && groupScope)) {
        throw new Error("会话客户端或档位参数无效");
      }
      const groupId = String(args?.groupId ?? "");
      const selected = demoGroups.find((group) => group.groupId === groupId);
      if (!selected) throw new Error("演示会话组不存在");
      return selected satisfies SessionGroupDetail;
    }
    case "preview_session_group_pair": {
      if (!groupClient || (groupClient === "codebuddyIde" && !groupScope) || (groupClient !== "codebuddyIde" && groupScope)) {
        throw new Error("会话客户端或档位参数无效");
      }
      const group = demoGroups.find((item) => item.groupId === args?.groupId);
      const sourceMemberId = String(args?.sourceMemberId ?? "");
      const targetMemberId = String(args?.targetMemberId ?? "");
      const source = group?.members.find((member) => member.memberId === sourceMemberId);
      const target = group?.members.find((member) => member.memberId === targetMemberId);
      if (!group || !source || !target || sourceMemberId === targetMemberId) throw new Error("来源或目标成员无效");
      const verdict = target.versionStatus === "behind" ? "fastForward" : target.versionStatus === "diverge" ? "diverge" : target.versionStatus === "missing" || target.versionStatus === "unknown" ? "unknown" : "identical";
      return {
        client: groupClient,
        variantScope: groupScope,
        groupId: group.groupId,
        sourceMemberId,
        targetMemberId,
        verdict,
        availableModes: verdict === "fastForward" ? ["fastForward"] : verdict === "diverge" ? ["overwrite"] : [],
        previewToken: verdict === "fastForward" || verdict === "diverge" ? `demo-preview-${group.groupId}-${targetMemberId}` : null,
        reason: target.reason,
        recordCount: { source: source.recordCount ?? 0, target: target.recordCount ?? 0, baseline: null },
        extraTargetCount: verdict === "diverge" ? 2 : 0,
      } satisfies SessionGroupPairPreview;
    }

    case "list_vscode_sessions": return {
      sourceUid: demoAccounts[0].uid,
      skipped: 0,
      dataRoot: "/demo/CodeBuddyExtension/Data",
      sessions: [
        { id: "7f3a91c0d4e5b6a7c8d9e0f1a2b3c4d5", workspaceHash: "3c1f8a92b4d5e60718f9a0b1c2d3e4f5", title: "完善账号卡片交互", updatedAt: Date.now() - 1000 * 60 * 12, type: "craft", hasHistory: true },
        { id: "9b8c7d6e5f4a3b2c1d0e9f8a7b6c5d4e", workspaceHash: "3c1f8a92b4d5e60718f9a0b1c2d3e4f5", title: "修复切换后历史为空", updatedAt: Date.now() - 1000 * 60 * 60 * 3, type: "craft", hasHistory: true },
        { id: "11223344556677889900112233445566", workspaceHash: "aabbccddeeff00112233445566778899", title: "设计会话复制方案", updatedAt: Date.now() - 1000 * 60 * 60 * 26, type: "craft", hasHistory: true },
        { id: "66554433221100998877665544332211", workspaceHash: "aabbccddeeff00112233445566778899", title: "(无标题)", updatedAt: Date.now() - 1000 * 60 * 60 * 50, type: "craft", hasHistory: false },
      ],
    } satisfies VscodeSessionList;
    // 国内版 / 国际版 IDE 共用同一套会话存储，演示数据也只有来源 uid 不同。
    case "list_codebuddy_ide_sessions": return demoIdeSessionList(demoAccounts[0].uid ?? "demo-source");
    case "list_codebuddy_intl_ide_sessions": return demoIdeSessionList(intlAccountA.uid ?? "demo-intl-source");

    // 关联预览：演示库没有复制记录，返回 missing（弹窗默认 tab 会拉一次；不得落到「演示模式不可操作」）。
    // 会话管理页的副本状态（演示）：三条会话分别展示「目标落后 / 已一致 / 两边都有改动」。
    case "session_links_preview_cross": {
      const source = demoAccounts.find((account) => account.id === args?.sourceAccountId) ?? demoAccounts[0];
      const target = demoAccounts.find((account) => account.id === args?.targetAccountId) ?? demoAccounts[1] ?? demoAccounts[0];
      return {
        supported: true,
        storeStatus: "ready",
        sourceUid: source.uid ?? "demo-source",
        targetUid: target.uid ?? "demo-target",
        sourceVariant: accountVariant(source),
        targetVariant: accountVariant(target),
        groups: [
          demoPreviewGroup("demo-sess-space-1", "跨档复制内核改造", "fastForward", 3, 0),
          demoPreviewGroup("demo-sess-space-2", "会话管理页骨架", "identical", 0, 0),
          demoPreviewGroup("demo-sess-task-1", "整理会话复制文案", "diverge", 4, 2),
        ],
      } satisfies SessionLinksPreview;
    }
    case "codebuddy_ide_session_links_preview":
    case "codebuddy_intl_ide_session_links_preview":
    case "vscode_session_links_preview": {
      const target = demoAccounts.find((account) => account.id === args?.targetAccountId) ?? demoAccounts[1] ?? demoAccounts[0];
      return {
        supported: true,
        storeStatus: "missing",
        sourceUid: demoAccounts[0].uid ?? "demo-source",
        targetUid: target.uid ?? "demo-target",
        groups: [],
      } satisfies SessionLinksPreview;
    }
    case "get_token_statistics": return demoTokenStatistics(typeof args?.days === "number" ? args.days : undefined);
    default: return undefined;
  }
}
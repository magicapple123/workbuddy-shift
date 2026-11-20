import { STATUS, sessionHeadline } from './model.js';
import { prolongedPermissionCheck } from './permission-check.js';
import { agentSessionLink, codeBuddyFolderLink, hasOpenableFolder, isCodeBuddyVSCodeHost, sessionBadge, sessionSourceLabel, type SessionBadge } from './session-link.js';
import type { ConnectionState, PendingKind, PendingRequest, PresentationStatus, Session, SourceHealth } from '../types/snapshot.js';

export interface SessionPresentation {
  status: PresentationStatus;
  statusLabel: string;
  title: string;
  provider: string;
  badge: SessionBadge | null;
  question: string;
  url: string | null;
  action: string;
}

/** The tool names the agent uses to ask the user a question, bare-name form. */
const QUESTION_TOOLS = new Set(['request_user_input', 'request_user_input_async', 'AskUserQuestion', 'ask_user_question', 'RequestUserInput']);

/**
 * What a pending request is waiting for.
 *
 * `kind` is stamped by the adapter (WorkBuddy stamps `permission` for a tool
 * gate and `question` for the question tool) and inferred by `hub.rs` when a
 * producer does not; this repeats the same inference so a snapshot captured by
 * an older runtime still renders with the right wording.
 */
export function pendingKind(pending?: PendingRequest | null): PendingKind | null {
  if (!pending) return null;
  if (pending.kind === 'permission' || pending.kind === 'question') return pending.kind;
  const tool = String(pending.tool || '');
  if (tool === 'permission' || tool === 'notification') return 'permission';
  const bare = tool.split('__').pop()!.split('.').pop()!;
  if (tool === 'ask' || QUESTION_TOOLS.has(bare) || (pending.questions?.length ?? 0) > 0) return 'question';
  // A gate with no question payload is what the user has to authorise.
  return 'permission';
}

/** Best-effort Chinese label for a tool name; unknown tools keep their own name. */
const TOOL_LABELS: Record<string, string> = {
  Bash: '执行命令', Shell: '执行命令', Terminal: '执行命令', RunCommand: '执行命令',
  Write: '写入文件', Edit: '修改文件', MultiEdit: '批量修改文件', NotebookEdit: '修改笔记本',
  Read: '读取文件', Delete: '删除文件', Remove: '删除文件', Move: '移动文件',
  WebFetch: '访问网页', WebSearch: '联网搜索', Task: '启动子任务',
  permission: '工具授权', notification: '工具授权',
};

export function permissionToolLabel(tool?: string): string {
  const name = String(tool || '').trim();
  if (!name) return '工具授权';
  const bare = name.split('__').pop()!.split('.').pop()!;
  return TOOL_LABELS[name] || TOOL_LABELS[bare] || bare || '工具授权';
}

// Shared by the office card and the desktop rail. No scene or DOM dependencies.
export function sessionPresentation(session?: Session | null, connection: ConnectionState = 'connected', sourceHealth?: SourceHealth): SessionPresentation {
  const unavailable = connection !== 'connected' || (sourceHealth && !['ok', 'partial'].includes(sourceHealth.state));
  const status = unavailable ? 'offline' : session?.status || 'idle';
  const pending = session?.pending?.[0];
  const kind = status === 'wait' ? pendingKind(pending) : null;
  const permission = kind === 'permission';
  const slowPermission = status === 'running' && !!session && prolongedPermissionCheck(session);
  const url = agentSessionLink(session);
  // Each host names the action after the link its own builder accepts, so the
  // button never promises an app the target does not open.
  const linkAction = session?.source !== 'codebuddy-ide' ? '进入对话'
    : isCodeBuddyVSCodeHost(session) ? (hasOpenableFolder(session.cwd) ? '打开工程' : '打开 VS Code')
      : (codeBuddyFolderLink(session.cwd, session) ? '打开工程' : '打开 CodeBuddy');
  // A permission gate is answered in the client, so the card offers a jump to
  // that conversation instead of pretending the rail can resolve it.
  const action = permission ? '去处理' : linkAction;
  const question = status === 'wait'
    ? permission
      ? [permissionToolLabel(pending?.tool), String(pending?.text || '').trim()].filter(Boolean).join(' · ')
      : pending?.questions?.[0]?.text || pending?.text || ''
    : slowPermission ? '请查看 Codex，可能仍在自动审批' : '';
  return {
    status,
    statusLabel: session?.endedBy === 'host' ? '已退出' : slowPermission ? '权限请求未完成' : permission ? '需要授权' : status === 'running' && session?.permissionChecks?.length ? '权限检查中' : STATUS[status] || status,
    title: sessionHeadline(session, status),
    provider: sessionSourceLabel(session),
    badge: sessionBadge(session),
    question,
    url,
    action,
  };
}

import type { ReactNode } from "react";
import { ChevronDown, ChevronRight } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import type { Session } from "@/lib/types";

/**
 * WorkBuddy 会话树（切号弹窗「复制会话」、「新增关联会话」弹窗共用）：
 * 任务（playground）平铺，空间按 `space_id` 分组（缺失时回退 cwd 最后一段），
 * 组头三态勾选。
 *
 * 结构与 WorkBuddy 侧栏一致，只有「空间 → 会话」两层：空间名来自 `spaces.name`
 * （后端按 `sessions.space_id` 关联下发），不再是工作目录的最后一段；工作目录退到
 * 会话行的 `title` 悬停里，不再单独占一层。
 *
 * 只负责展示与收集：勾选集合与展开集合都由调用方持有（与切号弹窗的既有契约一致）。
 */

/** 一个空间分组：`key` 是稳定分组键，同时也用作展开集合的键。 */
export type SpaceGroup = { key: string; label: string; sessions: Session[] };
export type KindGroup = {
  key: "task" | "space";
  label: string;
  /** 任务 = 会话数；空间 = 空间分组数（与改造前的「空间 (N)」口径一致）。 */
  count: number;
  sessions: Session[];
  spaces?: SpaceGroup[];
};

/** 空间分组键前缀：与 cwd 回退键分开，避免「空间 id 恰好等于某个 cwd」时串组。 */
const SPACE_KEY_PREFIX = "space:";
const CWD_KEY_PREFIX = "cwd:";
/**
 * 「未分组」的稳定组键：只给「库里能关联空间、但这条会话没有空间」的会话用。
 *
 * 用常量而不是 `cwd:` / `space:` 前缀，既不会和真实 `spaceId` 撞，也不会和任何目录名撞；
 * 该组固定排在最后。
 */
const UNGROUPED_KEY = "__ungrouped__";
/** 没有 cwd 也没有空间的会话统一落在这一格（与改造前文案一致）。 */
const NO_GROUP_LABEL = "未分组";

/**
 * WorkBuddy 自动生成工作目录的数据根目录名（`cwd` 的**直接**父目录）。
 *
 * 只列本工具已知的客户端数据根：国内版 `WorkBuddy_data`，macOS / 国际版
 * `WorkBuddy`、`WorkBuddy AI`。判定用父目录名，不猜盘符或用户目录，避免把
 * 「恰好也叫时间戳」的用户项目当成客户端目录。
 */
const WORKBUDDY_DATA_ROOT_NAMES = new Set(["workbuddy_data", "workbuddy", "workbuddy ai"]);
/** 客户端工作目录的时间戳形状：`2026-11-15-01-27-23`。 */
const WORKBUDDY_WORKSPACE_TIMESTAMP = /^\d{4}-\d{2}-\d{2}-\d{2}-\d{2}-\d{2}$/;

export function selectionState(sessions: Session[], selected: Set<string>) {
  const ids = sessions.map((s) => s.id);
  const n = ids.filter((id) => selected.has(id)).length;
  return { allOn: n === ids.length && ids.length > 0, someOn: n > 0 && n < ids.length };
}

export function TreeCheckbox({
  allOn,
  someOn,
  onChange,
  ariaLabel,
}: {
  allOn: boolean;
  someOn: boolean;
  onChange: () => void;
  ariaLabel: string;
}) {
  return (
    <input
      type="checkbox"
      className="size-3.5 shrink-0 cursor-pointer accent-primary"
      checked={allOn}
      ref={(el) => {
        if (el) el.indeterminate = someOn;
      }}
      onChange={onChange}
      aria-label={ariaLabel}
    />
  );
}

/** 会话行的悬停文案：标题 + 工作目录（目录从分组层退到 title 里）。 */
export function sessionCwdTitle(session: Session): string {
  const cwd = session.cwd.trim();
  return cwd ? `${session.title}\n${cwd}` : session.title;
}

export function SessionPickRow({
  session,
  checked,
  indentClass,
  onToggle,
  trailing,
}: {
  session: Session;
  checked: boolean;
  indentClass: string;
  onToggle: () => void;
  /** 行尾附加内容（会话管理页展示副本状态）；缺省不渲染。 */
  trailing?: ReactNode;
}) {
  return (
    <label
      className={`flex cursor-pointer items-center gap-2.5 rounded-md py-1.5 pr-2 hover:bg-accent/50 ${indentClass}`}
    >
      <input
        type="checkbox"
        className="size-3.5 shrink-0 cursor-pointer accent-primary"
        checked={checked}
        onChange={onToggle}
      />
      <span className="min-w-0 flex-1 truncate text-sm" title={sessionCwdTitle(session)}>
        {session.title}
      </span>
      {session.hasHistory && (
        <Badge variant="outline" className="shrink-0 text-[10px]">
          有内容
        </Badge>
      )}
      {trailing}
    </label>
  );
}

/** WorkBuddy 侧栏文件夹名：cwd 最后一段（没有空间时的回退标签）。 */
export function sessionFolderLabel(cwd: string): string {
  const normalized = cwd.trim().replace(/[\\/]+$/, "");
  if (!normalized) return NO_GROUP_LABEL;
  const parts = normalized.split(/[\\/]/);
  return parts[parts.length - 1] || normalized;
}

/** 会话所属空间 id；`space_id` 缺失或空白都当作没有（回退 cwd 分组）。 */
function sessionSpaceId(session: Session): string | null {
  const spaceId = session.spaceId?.trim();
  return spaceId ? spaceId : null;
}

/**
 * 是否是 WorkBuddy 自动生成的时间戳工作目录：`<数据根>/<yyyy-MM-dd-HH-mm-ss>`。
 *
 * 为什么只认「数据根**下一层**的纯时间戳目录」，不做通用规则：
 * - `D:/WorkBuddy_data/2026-11-15-01-27-23` → 是。客户端给无空间会话分配的临时工作目录，
 *   真实库里这类会话 `space_id` 为空，用时间戳当空间名正是本轮要修的误导。
 * - `D:/WorkBuddy_data/spaces/2026_11_16_13-46-13` → 不是（多一层，且下划线命名），
 *   这是真实空间的目录，空间名必须来自 `spaces.name`。
 * - `D:/projects/2026-11-15-01-27-23` → 不是（父目录不是客户端数据根），用户自己名叫
 *   时间戳的项目必须照旧按文件夹名分组。
 *
 * 三层同时命中才算：没有空间 + 父目录是已知数据根 + 末段是时间戳。放宽任何一条都会
 * 把用户的真实项目吞进「未分组」。
 */
export function isWorkBuddyTimestampDir(cwd: string): boolean {
  const segments = cwd.trim().split(/[\\/]+/).filter(Boolean);
  if (segments.length < 2) return false;
  const last = segments[segments.length - 1];
  const parent = segments[segments.length - 2];
  return (
    WORKBUDDY_WORKSPACE_TIMESTAMP.test(last) &&
    WORKBUDDY_DATA_ROOT_NAMES.has(parent.toLowerCase())
  );
}

/**
 * 分组键：有空间按空间；否则按 cwd 文件夹。
 *
 * 「未分组」只在**库支持空间**（后端下发了 `spaceId` 键）时启用：老库没有空间概念，
 * `cwd` 就是唯一线索，行为与改造前逐字一致。
 */
function sessionGroupKey(session: Session, spacesSupported: boolean): string {
  const spaceId = sessionSpaceId(session);
  if (spaceId) return `${SPACE_KEY_PREFIX}${spaceId}`;
  const cwd = session.cwd.trim();
  // 没有空间 + cwd 是客户端时间戳工作目录（或为空）：如实显示「未分组」，不冒充空间。
  if (spacesSupported && (cwd === "" || isWorkBuddyTimestampDir(cwd))) return UNGROUPED_KEY;
  return `${CWD_KEY_PREFIX}${cwd}`;
}

/**
 * 分组标签：`spaces.name` 优先；整组都拿不到空间名时回退 cwd 最后一段。
 *
 * 取整组第一个非空名字而不是只看首条：同一空间里个别会话的空间行缺失时，标签仍是
 * 真实空间名（后端对缺失/已删空间行返回 null）。
 */
function spaceGroupLabel(key: string, sessions: Session[]): string {
  const first = sessions[0];
  if (!first) return NO_GROUP_LABEL;
  if (key === UNGROUPED_KEY) return NO_GROUP_LABEL;
  if (key.startsWith(SPACE_KEY_PREFIX)) {
    const name = sessions.map((s) => s.spaceName?.trim()).find((value) => !!value);
    if (name) return name;
  }
  return sessionFolderLabel(first.cwd);
}

/**
 * 按空间分组：有 `spaceId` 就按空间合组，没有则回退 cwd 文件夹（老库行为不变）。
 *
 * 顺序沿用入参顺序（调用方已按 `updatedAt` 倒序），因此空间顺序就是最近活动顺序；
 * 「未分组」固定排最后。同名空间按 id 分成两组，**既不合并也不加后缀**：UI 判断不出
 * 两个同名空间是否是同一个项目，猜错会把不相关的会话勾到一起。
 */
export function groupSessionsBySpace(sessions: Session[]): SpaceGroup[] {
  // 后端下发 `spaceId` 键（哪怕是 null）才算支持空间；老库完全没有这个键。
  const spacesSupported = sessions.some((session) => session.spaceId !== undefined);
  const groups = new Map<string, Session[]>();
  const order: string[] = [];
  for (const session of sessions) {
    const key = sessionGroupKey(session, spacesSupported);
    let list = groups.get(key);
    if (!list) {
      list = [];
      groups.set(key, list);
      order.push(key);
    }
    list.push(session);
  }
  // 「未分组」永远排最后，不随最近活动时间上下浮动。
  const ordered = order.filter((key) => key !== UNGROUPED_KEY);
  if (groups.has(UNGROUPED_KEY)) ordered.push(UNGROUPED_KEY);
  return ordered.map((key) => {
    const list = groups.get(key) ?? [];
    return { key, label: spaceGroupLabel(key, list), sessions: list };
  });
}

/** 对齐 WorkBuddy 侧栏：任务（playground）平铺，其余按空间分组。 */
export function buildSessionTree(sessions: Session[]): KindGroup[] {
  const tasks = sessions.filter((s) => s.isPlayground);
  const spaces = sessions.filter((s) => !s.isPlayground);
  const groups: KindGroup[] = [];
  if (tasks.length > 0) {
    groups.push({ key: "task", label: "任务", count: tasks.length, sessions: tasks });
  }
  if (spaces.length > 0) {
    const spaceGroups = groupSessionsBySpace(spaces);
    groups.push({
      key: "space",
      label: "空间",
      count: spaceGroups.length,
      sessions: spaces,
      spaces: spaceGroups,
    });
  }
  return groups;
}

/**
 * 会话树列表：任务 / 空间（按 `space_id` 分组）分组，组头三态勾选与折叠。
 *
 * 勾选与展开集合由调用方持有；`renderTrailing` 用于行尾附加内容
 * （会话管理页展示副本状态），缺省与切号弹窗的渲染逐字一致。
 */
export function SessionTreeList({
  sessions,
  selected,
  expanded,
  onToggleSession,
  onToggleGroup,
  onToggleExpanded,
  renderTrailing,
  className,
}: {
  sessions: Session[];
  selected: Set<string>;
  /** 展开的节点：任务 / 空间 / 空间分组 key。 */
  expanded: Set<string>;
  onToggleSession: (id: string) => void;
  onToggleGroup: (ids: string[]) => void;
  onToggleExpanded: (key: string) => void;
  renderTrailing?: (session: Session) => ReactNode;
  className?: string;
}) {
  return (
    <div className={className}>
      {buildSessionTree(sessions).map((kind) => {
        const kindOpen = expanded.has(kind.key);
        const kindSel = selectionState(kind.sessions, selected);
        return (
          <div key={kind.key} className="mb-0.5">
            <div className="sticky top-0 z-10 flex items-center gap-1.5 rounded-md bg-background px-1.5 py-1">
              <TreeCheckbox
                allOn={kindSel.allOn}
                someOn={kindSel.someOn}
                onChange={() => onToggleGroup(kind.sessions.map((s) => s.id))}
                ariaLabel={`选择${kind.label}`}
              />
              <button
                type="button"
                className="flex min-w-0 flex-1 cursor-pointer items-center gap-1 rounded px-1 py-0.5 text-left hover:bg-accent/50"
                onClick={() => onToggleExpanded(kind.key)}
                aria-expanded={kindOpen}
                aria-label={`${kindOpen ? "折叠" : "展开"}${kind.label}`}
              >
                <span className="min-w-0 flex-1 truncate text-sm font-medium">
                  {kind.label}
                  <span className="ml-1 font-normal text-muted-foreground">({kind.count})</span>
                </span>
                {kindOpen ? (
                  <ChevronDown className="size-3.5 shrink-0 text-muted-foreground" />
                ) : (
                  <ChevronRight className="size-3.5 shrink-0 text-muted-foreground" />
                )}
              </button>
            </div>
            {kindOpen &&
              kind.key === "task" &&
              kind.sessions.map((s) => (
                <SessionPickRow
                  key={s.id}
                  session={s}
                  checked={selected.has(s.id)}
                  indentClass="pl-7"
                  onToggle={() => onToggleSession(s.id)}
                  trailing={renderTrailing?.(s)}
                />
              ))}
            {kindOpen &&
              kind.spaces?.map((space) => {
                const spaceOpen = expanded.has(space.key);
                const spaceSel = selectionState(space.sessions, selected);
                return (
                  <div key={space.key}>
                    <div className="flex items-center gap-1.5 px-1.5 py-0.5 pl-7">
                      <TreeCheckbox
                        allOn={spaceSel.allOn}
                        someOn={spaceSel.someOn}
                        onChange={() => onToggleGroup(space.sessions.map((s) => s.id))}
                        ariaLabel={`选择空间 ${space.label}`}
                      />
                      <button
                        type="button"
                        className="flex min-w-0 flex-1 cursor-pointer items-center gap-1 rounded px-1 py-0.5 text-left hover:bg-accent/50"
                        onClick={() => onToggleExpanded(space.key)}
                        aria-expanded={spaceOpen}
                        aria-label={`${spaceOpen ? "折叠" : "展开"}空间 ${space.label}`}
                      >
                        <span className="min-w-0 flex-1 truncate text-sm">{space.label}</span>
                        <span className="shrink-0 text-xs text-muted-foreground">
                          ({space.sessions.length})
                        </span>
                        {spaceOpen ? (
                          <ChevronDown className="size-3.5 shrink-0 text-muted-foreground" />
                        ) : (
                          <ChevronRight className="size-3.5 shrink-0 text-muted-foreground" />
                        )}
                      </button>
                    </div>
                    {spaceOpen &&
                      space.sessions.map((s) => (
                        <SessionPickRow
                          key={s.id}
                          session={s}
                          checked={selected.has(s.id)}
                          indentClass="pl-12"
                          onToggle={() => onToggleSession(s.id)}
                          trailing={renderTrailing?.(s)}
                        />
                      ))}
                  </div>
                );
              })}
          </div>
        );
      })}
    </div>
  );
}

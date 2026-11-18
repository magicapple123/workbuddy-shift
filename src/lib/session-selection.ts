/**
 * 会话勾选与展开集合的纯逻辑（切号弹窗 / 新增关联会话弹窗共用）。
 *
 * 这些函数只做不可变集合运算，不依赖 React：调用方用函数式更新传入旧集合即可，
 * 例如 `setSelected((prev) => toggleSessionSelection(prev, id))`。
 * 抽出来是为了让 WorkBuddy / VS Code / CodeBuddy IDE / 新增关联会话四处共用同一份
 * 「单选切换 / 组头三态勾选 / 展开折叠」语义，避免各弹窗各写一遍。
 */

/** 切换单个会话的勾选状态；返回新集合，不修改入参。 */
export function toggleSessionSelection(selected: ReadonlySet<string>, id: string): Set<string> {
  const next = new Set(selected);
  if (next.has(id)) next.delete(id);
  else next.add(id);
  return next;
}

/**
 * 组头三态勾选：组内已全选则整组取消，否则整组补选；空组不改变任何状态。
 *
 * 「部分勾选」按未全选处理（点一下补全），与组头半选复选框的直觉一致。
 */
export function toggleGroupSelection(
  selected: ReadonlySet<string>,
  ids: readonly string[],
): Set<string> {
  const next = new Set(selected);
  const allOn = ids.length > 0 && ids.every((id) => next.has(id));
  for (const id of ids) {
    if (allOn) next.delete(id);
    else next.add(id);
  }
  return next;
}

/** 切换一个折叠 / 展开节点的开关状态（按 key 去重）；返回新集合，不修改入参。 */
export function toggleSetKey(keys: ReadonlySet<string>, key: string): Set<string> {
  const next = new Set(keys);
  if (next.has(key)) next.delete(key);
  else next.add(key);
  return next;
}

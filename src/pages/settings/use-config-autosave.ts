import { useRef, useState } from "react";
import { toast } from "sonner";

import * as api from "@/lib/api";

import { numberInputHint, resolveNumberInput, type NumberFieldSpec, type SaveCommit } from "./helpers";

/** 待提交队列的纯状态：同一次交互里的连续触发合并为一份最新快照。 */
export interface SaveQueueState<T> {
  pending: SaveCommit<T> | null;
}

/**
 * 合并一次编辑到待提交队列。
 *
 * - 字段后写覆盖先写（开关 click 与输入框 blur 会先后触发，各自都带整份配置）；
 * - 成功后提示取最新一次，最新一次没给提示时沿用上一份。
 */
export function mergeSaveCommit<T>(
  pending: SaveCommit<T> | null,
  edits: Partial<T>,
  success?: SaveCommit<T>["success"],
): SaveCommit<T> {
  return { edits: { ...pending?.edits, ...edits }, success: success ?? pending?.success };
}

/** 入队：返回新的队列状态（不修改入参，便于单测与并发推演）。 */
export function enqueueSaveCommit<T>(
  state: SaveQueueState<T>,
  edits: Partial<T>,
  success?: SaveCommit<T>["success"],
): SaveQueueState<T> {
  return { pending: mergeSaveCommit(state.pending, edits, success) };
}

/** 出队：取出当前快照并清空队列；提交期间的新编辑会进入下一份快照。 */
export function takeSaveCommit<T>(state: SaveQueueState<T>): {
  state: SaveQueueState<T>;
  commit: SaveCommit<T> | null;
} {
  return { state: { pending: null }, commit: state.pending };
}

export interface ConfigAutosaveOptions<K extends string, T extends Record<K, number> & { enabled: boolean }> {
  /** 整份配置落盘并返回后端回读值。 */
  save: (config: T) => Promise<T>;
  /** 落盘失败的 toast 标题。 */
  saveErrorMessage: string;
  /** 数字参数定义：提交时按同一份 min/max 收敛，成功后提示用 label。 */
  numberFields: Record<K, NumberFieldSpec>;
  /** 主开关成功提示（开启 / 关闭）。 */
  enabledSuccessTitles: { on: string; off: string };
  /** 落盘回读的显示值；默认整份采用回读值（签到时间段需保留半填/非法草稿时由调用方覆盖）。 */
  mergeAdopted?: (saved: T, draft: T | null) => T;
}

export interface ConfigAutosaveController<K extends string, T extends Record<K, number> & { enabled: boolean }> {
  cfg: T | null;
  saving: boolean;
  numDraft: Partial<Record<K, string>>;
  /** 加载成功后初始化：记录已落盘基准并把回读值填进显示草稿。 */
  initialize: (saved: T) => void;
  /** 把后端回读值填进显示草稿（保留 mergeAdopted 声明的本地草稿）。 */
  adoptSaved: (saved: T) => void;
  /** 更新显示草稿：即时反馈只改本地，落盘走 enqueueSave。 */
  updateDraft: (next: T) => void;
  /** 当前显示草稿的最新同步值（事件回调里读，避免等到下一次渲染）。 */
  draft: () => T | null;
  /** 最近一次已确认落盘的配置（动作提交基准）。 */
  saved: () => T | null;
  /** 即时落盘：编辑先合并成一份最新快照，再串行提交。 */
  enqueueSave: (edits: Partial<T>, success?: SaveCommit<T>["success"]) => void;
  onToggleEnabled: (enabled: boolean) => void;
  /** 数字参数：失焦 / 回车提交；空值回退原值，越界收敛，与落盘值相同则不发请求。 */
  onNumberCommit: (key: K, raw: string) => void;
  /** 数字输入框：输入期间只改本地草稿文本，失焦 / 回车才提交。 */
  onNumberChange: (key: K, text: string) => void;
  clearNumDraft: (key: K) => void;
}

/**
 * 设置页「即时落盘配置」的共享状态机：draft / saved / pending / 串行提交链。
 *
 * 两个自动配置卡片（自动签到、自动轮换）此前各写一份同样的队列逻辑；这里只保留
 * 一份实现，差异（保存接口、失败文案、日志类型、时间段草稿保留规则）通过 options 注入。
 *
 * 队列语义：连续编辑合并成一份最新快照；提交串行，避免先发的那份（不含后一次编辑）
 * 后到达覆盖新值；提交期间出现新编辑时，回读值不整体覆盖显示。
 */
export function useConfigAutosave<K extends string, T extends Record<K, number> & { enabled: boolean }>(
  options: ConfigAutosaveOptions<K, T>,
): ConfigAutosaveController<K, T> {
  const [cfg, setCfg] = useState<T | null>(null);
  const [saving, setSaving] = useState(false);
  const [numDraft, setNumDraft] = useState<Partial<Record<K, string>>>({});
  /** 显示草稿的同步镜像：事件回调与异步回读都要读最新值，state 只负责渲染。 */
  const draftRef = useRef<T | null>(null);
  /** 最近一次落盘的配置：即时落盘以它为提交基准，未落盘的草稿不写盘。 */
  const savedRef = useRef<T | null>(null);
  const pendingRef = useRef<SaveQueueState<T>>({ pending: null });
  /** 提交链：串行发送，避免先发的那份后到达覆盖新值。 */
  const chainRef = useRef<Promise<void>>(Promise.resolve());
  const optionsRef = useRef(options);
  optionsRef.current = options;

  /** 更新显示草稿（state 供渲染，ref 供事件回调与异步回读读最新值）。 */
  function updateDraft(next: T) {
    draftRef.current = next;
    setCfg(next);
  }

  /** 落盘值回填显示：默认整份采用回读值，回读期间有编辑排队时不覆盖。 */
  function adoptSaved(next: T) {
    const draft = draftRef.current;
    const merge = optionsRef.current.mergeAdopted;
    updateDraft(merge ? merge(next, draft) : next);
  }

  function initialize(saved: T) {
    savedRef.current = saved;
    adoptSaved(saved);
  }

  function clearNumDraft(key: K) {
    setNumDraft((prev) => {
      const next = { ...prev };
      delete next[key];
      return next;
    });
  }

  async function flushSave() {
    const taken = takeSaveCommit(pendingRef.current);
    pendingRef.current = taken.state;
    const commit = taken.commit;
    const saved = savedRef.current;
    if (!commit || !saved) return;
    try {
      const next = await optionsRef.current.save({ ...saved, ...commit.edits });
      savedRef.current = next;
      // 回读值只在没有更新编辑排队时才整体覆盖显示，避免顶掉刚做出的改动。
      if (!pendingRef.current.pending) adoptSaved(next);
      if (commit.success) {
        toast.success(commit.success.title, { description: commit.success.description });
      }
    } catch (e) {
      adoptSaved(saved);
      toast.error(optionsRef.current.saveErrorMessage, { description: api.asError(e) });
    } finally {
      if (!pendingRef.current.pending) setSaving(false);
    }
  }

  function enqueueSave(edits: Partial<T>, success?: SaveCommit<T>["success"]) {
    if (!savedRef.current) return;
    pendingRef.current = enqueueSaveCommit(pendingRef.current, edits, success);
    setSaving(true);
    chainRef.current = chainRef.current.then(flushSave);
  }

  /** 主开关：拨动即落盘，失败回滚到上一份已确认配置。 */
  function onToggleEnabled(enabled: boolean) {
    const current = draftRef.current;
    if (!current) return;
    updateDraft({ ...current, enabled });
    const edits: Partial<T> = {};
    Object.assign(edits, { enabled });
    enqueueSave(
      edits,
      { title: enabled ? optionsRef.current.enabledSuccessTitles.on : optionsRef.current.enabledSuccessTitles.off },
    );
  }

  function onNumberCommit(key: K, raw: string) {
    const current = draftRef.current;
    const saved = savedRef.current;
    if (!current || !saved) return;
    const field = optionsRef.current.numberFields[key];
    const value = resolveNumberInput(raw, field);
    clearNumDraft(key);
    const next: T = { ...current };
    Object.assign(next, { [key]: value ?? saved[key] });
    updateDraft(next);
    if (value === null) {
      toast.error(numberInputHint(field));
      return;
    }
    if (value === saved[key]) return;
    const edits: Partial<T> = {};
    Object.assign(edits, { [key]: value });
    enqueueSave(edits, { title: `${field.label}已保存` });
  }

  function onNumberChange(key: K, text: string) {
    setNumDraft((prev) => ({ ...prev, [key]: text }));
  }

  return {
    cfg,
    saving,
    numDraft,
    initialize,
    adoptSaved,
    updateDraft,
    draft: () => draftRef.current,
    saved: () => savedRef.current,
    enqueueSave,
    onToggleEnabled,
    onNumberCommit,
    onNumberChange,
    clearNumDraft,
  };
}

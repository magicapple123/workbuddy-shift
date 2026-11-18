import { useCallback, useEffect, useRef, useState } from "react";

/**
 * localStorage 偏好读写的统一实现。
 *
 * 目标：默认值 / 损坏数据 / 存储不可用（隐私模式、受限 WebView）三件事只写一次；
 * 同一页面内的变更用 CustomEvent 通知，其它标签页用原生 storage 事件同步。
 * 调用方只需提供键名、默认值与序列化规则，且在模块级定义 options（对象引用稳定）。
 */
export interface PersistedPreferenceOptions<T> {
  /** localStorage 键名。 */
  key: string;
  /** 缺省 / 损坏 / 读取失败时的回退值。 */
  fallback: T;
  /** 存储文本 → 值；返回 null 表示非法，回退 fallback。 */
  parse: (raw: string) => T | null;
  /** 值 → 存储文本。 */
  serialize: (value: T) => string;
  /** 同页面内的变更通知事件名（其它标签页走 storage）。 */
  eventName: string;
}

/** 布尔偏好的存储格式：写 "1" / "0"；其它文本一律视为损坏（回退默认值）。 */
export function parseStoredBoolean(raw: string): boolean | null {
  if (raw === "1") return true;
  if (raw === "0") return false;
  return null;
}

/** 布尔偏好的标准 options：键名 + 同页事件名 + 默认值。 */
export function booleanPreference(
  key: string,
  eventName: string,
  fallback: boolean,
): PersistedPreferenceOptions<boolean> {
  return { key, fallback, parse: parseStoredBoolean, serialize: (value) => (value ? "1" : "0"), eventName };
}

/** 读取偏好：存储不可用、缺省或值损坏时返回 fallback，不抛错。 */
export function readPreference<T>(options: PersistedPreferenceOptions<T>): T {
  if (typeof window === "undefined") return options.fallback;
  try {
    const raw = localStorage.getItem(options.key);
    if (raw === null) return options.fallback;
    return options.parse(raw) ?? options.fallback;
  } catch {
    return options.fallback;
  }
}

/**
 * 写入偏好并通知同页面订阅者；返回是否真的写进了 localStorage。
 *
 * 写入失败（隐私模式等）静默降级且不广播：调用方的会话内 state 仍然生效，
 * 只是下次启动不恢复；若广播会让订阅者按存储回退值把本地状态覆盖回去。
 */
export function writePreference<T>(options: PersistedPreferenceOptions<T>, value: T): boolean {
  if (typeof window === "undefined") return false;
  let stored = false;
  try {
    localStorage.setItem(options.key, options.serialize(value));
    stored = true;
  } catch {
    /* 隐私模式等场景下写入失败：开关本次会话内仍生效（由调用方 state 驱动） */
  }
  if (stored) window.dispatchEvent(new Event(options.eventName));
  return stored;
}

/** 订阅同页事件 + 跨标签页 storage 事件；返回取消订阅函数。 */
export function subscribePreferenceChanges(eventName: string, listener: () => void): () => void {
  if (typeof window === "undefined") return () => {};
  const sync = () => listener();
  window.addEventListener("storage", sync);
  window.addEventListener(eventName, sync);
  return () => {
    window.removeEventListener("storage", sync);
    window.removeEventListener(eventName, sync);
  };
}

/** 订阅某个偏好的变化（同页事件名取自 options）。 */
export function subscribePreference<T>(
  options: PersistedPreferenceOptions<T>,
  listener: () => void,
): () => void {
  return subscribePreferenceChanges(options.eventName, listener);
}

/** 订阅偏好并返回当前值；同页与其它标签页的变更都会重新读取。 */
export function usePersistedPreference<T>(options: PersistedPreferenceOptions<T>): T {
  return usePersistedPreferenceState(options)[0];
}

/**
 * 订阅偏好并返回 [当前值, 写入函数]。
 *
 * 写入函数先更新本地 state 再落盘：存储不可用时本次会话内仍可切换（与原实现一致），
 * 落盘失败不广播同页事件，避免订阅者按存储回退值覆盖刚做出的选择。
 */
export function usePersistedPreferenceState<T>(
  options: PersistedPreferenceOptions<T>,
): [T, (value: T) => void] {
  const [value, setValue] = useState<T>(() => readPreference(options));
  const optionsRef = useRef(options);
  optionsRef.current = options;
  useEffect(() => {
    const sync = () => setValue(readPreference(optionsRef.current));
    return subscribePreference(options, sync);
  }, [options.key, options.eventName]);
  const update = useCallback((next: T) => {
    setValue(next);
    writePreference(optionsRef.current, next);
  }, []);
  return [value, update];
}
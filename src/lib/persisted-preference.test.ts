import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  booleanPreference,
  parseStoredBoolean,
  readPreference,
  subscribePreferenceChanges,
  writePreference,
} from "./persisted-preference";

describe("persisted-preference", () => {
  const store = new Map<string, string>();
  const originalLocalStorage = globalThis.localStorage;
  const originalWindow = globalThis.window;
  const PREF = booleanPreference("workbuddy-shift.test-flag", "workbuddy-shift:test-flag-changed", true);

  const fakeLocalStorage = {
    getItem: (key: string) => store.get(key) ?? null,
    setItem: (key: string, val: string) => {
      store.set(key, String(val));
    },
    removeItem: (key: string) => {
      store.delete(key);
    },
    clear: () => store.clear(),
    get length() {
      return store.size;
    },
    key: (index: number) => Array.from(store.keys())[index] ?? null,
  };

  beforeEach(() => {
    store.clear();
    Object.defineProperty(globalThis, "localStorage", { value: fakeLocalStorage, writable: true, configurable: true });
    Object.defineProperty(globalThis, "window", { value: new EventTarget(), writable: true, configurable: true });
  });

  afterEach(() => {
    store.clear();
    Object.defineProperty(globalThis, "localStorage", { value: originalLocalStorage, writable: true, configurable: true });
    Object.defineProperty(globalThis, "window", { value: originalWindow, writable: true, configurable: true });
  });

  it("缺省时返回默认值", () => {
    expect(readPreference(PREF)).toBe(true);
  });

  it("读到合法值后返回存储值（默认值不生效）", () => {
    store.set(PREF.key, "0");
    expect(readPreference(PREF)).toBe(false);
  });

  it("损坏数据回退默认值，不把任意文本当布尔", () => {
    store.set(PREF.key, "true");
    expect(readPreference(PREF)).toBe(true); // fallback true
    store.set(PREF.key, "2");
    expect(readPreference(PREF)).toBe(true);
    const fallbackFalse = booleanPreference(PREF.key, PREF.eventName, false);
    expect(readPreference(fallbackFalse)).toBe(false);
  });

  it("parseStoredBoolean 只接受 1 / 0", () => {
    expect(parseStoredBoolean("1")).toBe(true);
    expect(parseStoredBoolean("0")).toBe(false);
    expect(parseStoredBoolean("")).toBeNull();
    expect(parseStoredBoolean("yes")).toBeNull();
  });

  it("写入按 serialize 落盘并分发同页通知事件", () => {
    const handler = vi.fn();
    globalThis.window.addEventListener(PREF.eventName, handler);
    try {
      writePreference(PREF, false);
      expect(store.get(PREF.key)).toBe("0");
      expect(handler).toHaveBeenCalledTimes(1);
    } finally {
      globalThis.window.removeEventListener(PREF.eventName, handler);
    }
  });

  it("订阅后同页事件与 storage 事件都会触发回调，取消订阅后不再触发", () => {
    const handler = vi.fn();
    const unsubscribe = subscribePreferenceChanges(PREF.eventName, handler);
    globalThis.window.dispatchEvent(new Event(PREF.eventName));
    globalThis.window.dispatchEvent(new Event("storage"));
    expect(handler).toHaveBeenCalledTimes(2);
    unsubscribe();
    globalThis.window.dispatchEvent(new Event(PREF.eventName));
    expect(handler).toHaveBeenCalledTimes(2);
  });

  it("localStorage 读取抛错时回退默认值，不抛出", () => {
    Object.defineProperty(globalThis, "localStorage", {
      value: {
        getItem: () => {
          throw new Error("SecurityError: Access is denied");
        },
      },
      writable: true,
      configurable: true,
    });
    expect(readPreference(PREF)).toBe(true);
  });

  it("localStorage 写入抛错时不抛出且不广播（避免覆盖会话内状态）", () => {
    Object.defineProperty(globalThis, "localStorage", {
      value: {
        setItem: () => {
          throw new Error("QuotaExceededError");
        },
      },
      writable: true,
      configurable: true,
    });
    const handler = vi.fn();
    globalThis.window.addEventListener(PREF.eventName, handler);
    try {
      expect(writePreference(PREF, true)).toBe(false);
      expect(handler).not.toHaveBeenCalled();
    } finally {
      globalThis.window.removeEventListener(PREF.eventName, handler);
    }
  });
});
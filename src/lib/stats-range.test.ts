import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { readPreference } from "@/lib/persisted-preference";
import {
  DEFAULT_STATS_RANGE,
  STATS_RANGE_PREFERENCE,
  statsRangeOptions,
  type StatsRangeKey,
} from "@/lib/stats-range";

describe("STATS_RANGE_PREFERENCE", () => {
  const store = new Map<string, string>();
  const originalLocalStorage = globalThis.localStorage;
  const originalWindow = globalThis.window;

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

  it("选项表里的所有键（含 90d 与 all）都能原样解析，parse 与 serialize 互逆", () => {
    const keys: StatsRangeKey[] = ["today", "7d", "30d", "month", "90d", "all"];
    expect(statsRangeOptions(true).map((option) => option.key)).toEqual(keys);
    for (const key of keys) {
      const stored = STATS_RANGE_PREFERENCE.serialize(key);
      expect(stored).toBe(key);
      expect(STATS_RANGE_PREFERENCE.parse(stored)).toBe(key);
    }
  });

  it("非法值、空字符串、大小写变体与带空白的值返回 null", () => {
    for (const raw of ["", "90D", "ALL", "Today", " 30d", "30d ", "90days", "alll", "null", "undefined", "{}"]) {
      expect(STATS_RANGE_PREFERENCE.parse(raw)).toBeNull();
    }
  });

  it("key 使用 workbuddy-shift. 前缀，eventName 使用 workbuddy-shift: 前缀", () => {
    expect(STATS_RANGE_PREFERENCE.key).toMatch(/^workbuddy-shift\./);
    expect(STATS_RANGE_PREFERENCE.eventName).toMatch(/^workbuddy-shift:/);
    expect(STATS_RANGE_PREFERENCE.key).toBe("workbuddy-shift.stats.range");
    expect(STATS_RANGE_PREFERENCE.eventName).toBe("workbuddy-shift:stats-range-changed");
  });

  it("fallback 等于 DEFAULT_STATS_RANGE（30 天）", () => {
    expect(STATS_RANGE_PREFERENCE.fallback).toBe(DEFAULT_STATS_RANGE);
    expect(DEFAULT_STATS_RANGE).toBe("30d");
  });

  it("存储缺省或损坏时读取回退默认值，合法值原样读出", () => {
    expect(readPreference(STATS_RANGE_PREFERENCE)).toBe("30d");
    store.set(STATS_RANGE_PREFERENCE.key, "corrupted");
    expect(readPreference(STATS_RANGE_PREFERENCE)).toBe("30d");
    store.set(STATS_RANGE_PREFERENCE.key, "90d");
    expect(readPreference(STATS_RANGE_PREFERENCE)).toBe("90d");
  });
});

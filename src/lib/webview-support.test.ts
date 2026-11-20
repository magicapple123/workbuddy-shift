import { describe, expect, it } from "vitest";

// `?raw` 由 Vite/vitest 提供，直接拿到 index.html 的文本（无需 node 类型）。
import indexHtml from "../../index.html?raw";

import {
  compareWebviewVersions,
  degradedNoticeMessage,
  degradedNoticeStorageKey,
  dismissDegradedNotice,
  evaluateWebviewSupport,
  isDegradedNoticeDismissed,
  isWebviewVersionBelow,
  parseWebviewVersion,
  probeWebviewCapabilities,
  shouldBlockRender,
  shouldShowDegradedNotice,
  webviewSupportFromVersion,
  webviewVersionToken,
  type WebviewCapabilities,
  type WebviewStorage,
} from "@/lib/webview-support";

const FULL: WebviewCapabilities = { hasLayer: true, hasProperty: true, hasColorMix: true };
const NO_LAYER: WebviewCapabilities = { hasLayer: false, hasProperty: true, hasColorMix: true };
const NO_PROPERTY: WebviewCapabilities = { hasLayer: true, hasProperty: false, hasColorMix: true };
const NO_MIX: WebviewCapabilities = { hasLayer: true, hasProperty: true, hasColorMix: false };

/** 简易内存存储，替代 localStorage。 */
function memoryStorage(initial: Record<string, string> = {}): WebviewStorage & {
  dump(): Record<string, string>;
} {
  const map = new Map(Object.entries(initial));
  return {
    getItem: (key) => map.get(key) ?? null,
    setItem: (key, value) => {
      map.set(key, value);
    },
    dump: () => Object.fromEntries(map),
  };
}

/** 会抛错的存储：模拟隐私模式 / 受限 WebView。 */
const BROKEN_STORAGE: WebviewStorage = {
  getItem: () => {
    throw new Error("storage disabled");
  },
  setItem: () => {
    throw new Error("storage disabled");
  },
};

describe("能力探测 → 档位（R5-17）", () => {
  it("三项齐备 → ok", () => {
    expect(evaluateWebviewSupport(FULL)).toBe("ok");
  });

  it("缺 @layer → blocked（整块丢弃 → 裸界面）", () => {
    expect(evaluateWebviewSupport(NO_LAYER)).toBe("blocked");
  });

  it("缺 CSS.registerProperty → degraded", () => {
    expect(evaluateWebviewSupport(NO_PROPERTY)).toBe("degraded");
  });

  it("缺 color-mix() → degraded", () => {
    expect(evaluateWebviewSupport(NO_MIX)).toBe("degraded");
  });

  it("缺 @layer 时优先判 blocked（即使另外两项也缺）", () => {
    expect(evaluateWebviewSupport({ hasLayer: false, hasProperty: false, hasColorMix: false })).toBe(
      "blocked",
    );
  });

  it("blocked 才阻断渲染，degraded / ok 都正常挂载", () => {
    expect(shouldBlockRender("blocked")).toBe(true);
    expect(shouldBlockRender("degraded")).toBe(false);
    expect(shouldBlockRender("ok")).toBe(false);
  });

  it("只有 degraded 显示提醒条", () => {
    expect(shouldShowDegradedNotice("degraded")).toBe(true);
    expect(shouldShowDegradedNotice("ok")).toBe(false);
    expect(shouldShowDegradedNotice("blocked")).toBe(false);
  });
});

describe("版本号比较与版本分档边界", () => {
  it("比较 major.minor", () => {
    expect(compareWebviewVersions("15.3", "15.4")).toBeLessThan(0);
    expect(compareWebviewVersions("15.4", "15.4")).toBe(0);
    expect(compareWebviewVersions("16.4", "16.3")).toBeGreaterThan(0);
    expect(compareWebviewVersions("16", "16.0")).toBe(0);
  });

  it("版本分档边界：15.3 / 15.4 / 16.3 / 16.4", () => {
    expect(webviewSupportFromVersion("15.3")).toBe("blocked");
    expect(webviewSupportFromVersion("15.4")).toBe("degraded");
    expect(webviewSupportFromVersion("16.3")).toBe("degraded");
    expect(webviewSupportFromVersion("16.4")).toBe("ok");
    expect(webviewSupportFromVersion("17.0")).toBe("ok");
  });

  it("取不到版本时不做版本判断", () => {
    expect(webviewSupportFromVersion(null)).toBe("unknown");
    expect(isWebviewVersionBelow(null, "15.4")).toBe(false);
  });

  it("非法版本号不抛错（按 0 处理）", () => {
    expect(compareWebviewVersions("abc", "15.4")).toBeLessThan(0);
    expect(() => compareWebviewVersions("", "")).not.toThrow();
  });
});

describe("UA 解析与存储键", () => {
  const safari163 =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.3 Safari/605.1.15";
  const wkwebview =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko)";
  const windows =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36 Edg/120.0.0.0";

  it("Safari UA 取 Version 与 AppleWebKit", () => {
    expect(parseWebviewVersion(safari163)).toEqual({ safari: "16.3", appleWebKit: "605.1.15" });
  });

  it("WKWebView UA 只有 AppleWebKit", () => {
    expect(parseWebviewVersion(wkwebview)).toEqual({ safari: null, appleWebKit: "605.1.15" });
  });

  it("未知 UA 两项都为 null", () => {
    expect(parseWebviewVersion("curl/8.0")).toEqual({ safari: null, appleWebKit: null });
  });

  it("存储键后缀优先 Safari 版本，其次 WebKit，最后 unknown", () => {
    expect(webviewVersionToken(safari163)).toBe("16.3");
    expect(webviewVersionToken(wkwebview)).toBe("605.1.15");
    expect(webviewVersionToken("curl/8.0")).toBe("unknown");
    expect(degradedNoticeStorageKey("16.3")).toBe("wb-shift.webview-notice.dismissed:16.3");
  });

  it("提醒文案带上检测到的版本", () => {
    expect(degradedNoticeMessage(safari163)).toContain("Safari 16.3");
    expect(degradedNoticeMessage(safari163)).toContain("16.4");
    // Chromium 系 UA 没有 Version/，回落到 AppleWebKit 版本
    expect(degradedNoticeMessage(windows)).toContain("WebKit 537.36");
    // 两项都取不到时才写「未知版本」
    expect(degradedNoticeMessage("curl/8.0")).toContain("未知版本");
  });
});

describe("关闭提醒的读写（含存储不可用）", () => {
  it("默认未关闭", () => {
    const storage = memoryStorage();
    expect(isDegradedNoticeDismissed("16.3", storage)).toBe(false);
  });

  it("关闭后置位，且只影响该版本", () => {
    const storage = memoryStorage();
    dismissDegradedNotice("16.3", storage);
    expect(isDegradedNoticeDismissed("16.3", storage)).toBe(true);
    // 升级 Safari 后版本变化 → 重新提示一次
    expect(isDegradedNoticeDismissed("16.4", storage)).toBe(false);
    expect(storage.dump()).toEqual({ "wb-shift.webview-notice.dismissed:16.3": "1" });
  });

  it("存储不可用时不抛错，且按未关闭处理", () => {
    expect(() => dismissDegradedNotice("16.3", BROKEN_STORAGE)).not.toThrow();
    expect(isDegradedNoticeDismissed("16.3", BROKEN_STORAGE)).toBe(false);
  });

  it("无存储（null）时同样安全", () => {
    expect(isDegradedNoticeDismissed("16.3", null)).toBe(false);
    expect(() => dismissDegradedNotice("16.3", null)).not.toThrow();
  });
});

describe("probeWebviewCapabilities", () => {
  it("全支持的窗口对象", () => {
    const target = {
      CSSLayerBlockRule: function CSSLayerBlockRule() {},
      CSS: { registerProperty: () => {}, supports: () => true },
    };
    expect(probeWebviewCapabilities(target)).toEqual({
      hasLayer: true,
      hasProperty: true,
      hasColorMix: true,
    });
  });

  it("什么都没有的宿主按全缺处理", () => {
    expect(probeWebviewCapabilities({})).toEqual({
      hasLayer: false,
      hasProperty: false,
      hasColorMix: false,
    });
  });

  it("CSS.supports 返回 false → 缺 color-mix", () => {
    const target = {
      CSSLayerBlockRule: function CSSLayerBlockRule() {},
      CSS: { registerProperty: () => {}, supports: () => false },
    };
    expect(probeWebviewCapabilities(target).hasColorMix).toBe(false);
  });

  it("null / undefined 入参不抛错", () => {
    expect(() => probeWebviewCapabilities(null)).not.toThrow();
    expect(() => probeWebviewCapabilities(undefined)).not.toThrow();
  });
});

describe("index.html 内联脚本与 TS 判定保持一致", () => {
  const html: string = indexHtml;

  it("内联脚本探测三项能力", () => {
    expect(html).toContain("CSSLayerBlockRule");
    expect(html).toContain("CSS.registerProperty");
    expect(html).toContain("color-mix(in srgb, red, blue)");
  });

  it("内联脚本写入档位供 main.tsx 读取", () => {
    expect(html).toContain("window.__WB_SUPPORT__");
    expect(html).toMatch(/"blocked"/);
    expect(html).toMatch(/"degraded"/);
  });

  it("阻断页在 DOM 上可判定（data-webview-blocked）", () => {
    expect(html).toContain("data-webview-blocked");
  });

  it("首屏兜底脚本不会顶掉阻断页", () => {
    expect(html).toContain('window.__WB_SUPPORT__ === "blocked"');
  });
});

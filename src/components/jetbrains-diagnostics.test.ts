import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";

import { classifyJetbrainsState, JetbrainsDiagnosticsPanel } from "./jetbrains-diagnostics";
import type { JetbrainsDiagnostics, JetbrainsStatus } from "@/lib/types";

/**
 * 第 16 条验收口径：状态必须能区分「未安装 / 已安装未运行 / 运行中但版本不可识别 / 可切换」，
 * 且每种状态的下一步是可读的。
 *
 * 组件无 DOM 依赖地走 `renderToStaticMarkup`（与现有 Vitest node 环境一致）。
 */

function dir(overrides: Partial<JetbrainsDiagnostics["scannedDirs"][number]> = {}) {
  return {
    name: "IntelliJIdea2026.3",
    accepted: true,
    pluginDirs: ["coding-copilot-jetbrains"],
    pluginInstalled: true,
    hasSecretFile: true,
    ...overrides,
  };
}

function diagnostics(overrides: Partial<JetbrainsDiagnostics> = {}): JetbrainsDiagnostics {
  const scannedDirs = overrides.scannedDirs ?? [dir()];
  return {
    configRoot: "C:/Users/demo/AppData/Roaming/JetBrains",
    configRootExists: true,
    scannedDirs,
    acceptedDirs: scannedDirs.filter((item) => item.accepted).map((item) => item.name),
    pluginInstalledDirs: scannedDirs.filter((item) => item.pluginInstalled).map((item) => item.name),
    running: [],
    unmappedRunningCount: 0,
    ...overrides,
  };
}

function status(overrides: Partial<JetbrainsStatus> = {}): JetbrainsStatus {
  return {
    installed: true,
    pluginInstalled: true,
    running: false,
    loggedIn: true,
    configRoot: "C:/Users/demo/AppData/Roaming/JetBrains",
    targets: [],
    activeAccountId: null,
    activeAccountName: null,
    diagnostics: diagnostics(),
    ...overrides,
  };
}

describe("classifyJetbrainsState", () => {
  it("没有配置根或没有任何子目录 → 未安装", () => {
    const empty = status({ diagnostics: diagnostics({ configRootExists: false, scannedDirs: [] }) });
    expect(classifyJetbrainsState(empty).kind).toBe("missing");
    expect(classifyJetbrainsState(empty).hint).toContain("安装");

    // 配置根存在但下面空着：同样算「未安装」。
    const noDirs = status({ diagnostics: diagnostics({ scannedDirs: [] }) });
    expect(classifyJetbrainsState(noDirs).kind).toBe("missing");

    // 状态还没拿到时不能说「可切换」。
    expect(classifyJetbrainsState(null).kind).toBe("missing");
  });

  it("目录名全部不被接受 → 未装插件（带目录名，便于反馈）", () => {
    const rejected = status({
      pluginInstalled: false,
      diagnostics: diagnostics({
        scannedDirs: [dir({ name: "PyCharm2026.2-backup", accepted: false })],
      }),
    });
    const summary = classifyJetbrainsState(rejected);
    expect(summary.kind).toBe("no-plugin");
    expect(summary.title).toContain("不是受支持的产品名");
    expect(summary.hint).toContain("PyCharm2026.2-backup");
  });

  it("IDE 已安装但没装插件 → 未装插件（提示去插件市场装）", () => {
    const noPlugin = status({
      pluginInstalled: false,
      diagnostics: diagnostics({ scannedDirs: [dir({ pluginDirs: [], pluginInstalled: false })] }),
    });
    const summary = classifyJetbrainsState(noPlugin);
    expect(summary.kind).toBe("no-plugin");
    expect(summary.hint).toContain("Tencent Cloud CodeBuddy");
  });

  it("装了插件但 IDE 没运行 → 可切换（写入后下次打开生效）", () => {
    const idle = status({ running: false });
    const summary = classifyJetbrainsState(idle);
    expect(summary.kind).toBe("idle");
    expect(summary.hint).toContain("下次打开");
  });

  it("装了插件且 IDE 在运行 → 可切换（先退出再重开）", () => {
    const running = status({ running: true });
    const summary = classifyJetbrainsState(running);
    expect(summary.kind).toBe("ready");
    expect(summary.hint).toContain("重新打开");
  });

  it("后端不返回 diagnostics（旧版本 / 演示数据）时不误报未安装", () => {
    const legacy = status({ diagnostics: null, installed: true, pluginInstalled: true, running: false });
    expect(classifyJetbrainsState(legacy).kind).toBe("idle");

    // installed=false 时仍应判为未安装。
    const none = status({ diagnostics: null, installed: false, pluginInstalled: false });
    expect(classifyJetbrainsState(none).kind).toBe("missing");
  });

  it("运行中但映射不到配置目录 → 阻断（优先级最高）", () => {
    const blocked = status({
      running: true,
      unmappedRunning: [
        { pid: 4242, product: "IntelliJ IDEA", version: "2026.3", path: "D:/idea/bin/idea64.exe", reason: "product-info.json 不存在" },
      ],
    });
    const summary = classifyJetbrainsState(blocked);
    expect(summary.kind).toBe("blocked");
    expect(summary.title).toContain("1 个");
    expect(summary.hint).toContain("完全退出");
  });
});

describe("JetbrainsDiagnosticsPanel", () => {
  it("异常态默认就把结论显示出来，细节仍收在折叠区里", () => {
    const markup = renderToStaticMarkup(
      createElement(JetbrainsDiagnosticsPanel, {
        status: status({ pluginInstalled: false, diagnostics: diagnostics({ scannedDirs: [] }) }),
      }),
    );
    expect(markup).toContain('data-state="missing"');
    expect(markup).toContain("未检测到 JetBrains IDE");
    expect(markup).toContain("<details");
    expect(markup).not.toContain('open=""'); // 细节不该默认展开
  });

  it("需要的场景（没有可切换目标）默认展开诊断明细", () => {
    const markup = renderToStaticMarkup(
      createElement(JetbrainsDiagnosticsPanel, {
        status: status({ pluginInstalled: false, diagnostics: diagnostics({ scannedDirs: [] }) }),
        defaultOpen: true,
      }),
    );
    expect(markup).toContain('open=""');
    expect(markup).toContain("配置根");
  });

  it("一切正常时不额外顶一行结论，但明细仍可展开", () => {
    const markup = renderToStaticMarkup(
      createElement(JetbrainsDiagnosticsPanel, { status: status({ running: true }) }),
    );
    expect(markup).not.toContain("data-state=");
    expect(markup).toContain("<details");
    expect(markup).toContain("IntelliJIdea2026.3");
  });
});

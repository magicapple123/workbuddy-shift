import { describe, expect, it } from "vitest";

import { productLabel } from "./jetbrains-switch-dialog";

/**
 * R5-19：切换弹窗要把配置目录名翻成可读标签（`AndroidStudio2024.1` →
 * `Android Studio 2024.1`）。前缀表顺序敏感——`PyCharmCE` 必须排在 `PyCharm` 前面，
 * 否则社区版会被显示成 `PyCharm CE2025.1`（错位）甚至 `PyCharm CE2025.1` 之外的形态。
 */
describe("productLabel", () => {
  it("16 个产品都能翻成展示名", () => {
    for (const [configDir, expected] of [
      ["IntelliJIdea2026.3", "IntelliJ IDEA 2026.3"],
      ["IdeaIC2025.1", "IntelliJ IDEA CE 2025.1"],
      ["PyCharm2026.2", "PyCharm 2026.2"],
      ["PyCharmCE2025.1", "PyCharm CE 2025.1"],
      ["WebStorm2025.2", "WebStorm 2025.2"],
      ["PhpStorm2025.2", "PhpStorm 2025.2"],
      ["GoLand2025.2", "GoLand 2025.2"],
      ["CLion2025.2", "CLion 2025.2"],
      ["RubyMine2025.2", "RubyMine 2025.2"],
      ["DataGrip2025.2", "DataGrip 2025.2"],
      ["DataSpell2025.2", "DataSpell 2025.2"],
      ["Rider2025.2", "Rider 2025.2"],
      ["Aqua2025.2", "Aqua 2025.2"],
      ["AppCode2023.1", "AppCode 2023.1"],
      ["MPS2024.1", "MPS 2024.1"],
      ["AndroidStudio2024.1", "Android Studio 2024.1"],
    ] as const) {
      expect(productLabel(configDir)).toBe(expected);
    }
  });

  it("长前缀优先：PyCharmCE / IdeaIC 不被短前缀抢走", () => {
    expect(productLabel("PyCharmCE2025.1")).toBe("PyCharm CE 2025.1");
    expect(productLabel("IdeaIC2026.3")).toBe("IntelliJ IDEA CE 2026.3");
    // EAP / nightly 目录名照样只切前缀。
    expect(productLabel("GoLand2025.2-eap")).toBe("GoLand 2025.2-eap");
  });

  it("认不出的目录名原样返回（诊断场景保留真实名字）", () => {
    expect(productLabel("Unknown2026.1")).toBe("Unknown2026.1");
    expect(productLabel("")).toBe("");
  });
});

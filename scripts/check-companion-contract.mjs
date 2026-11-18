#!/usr/bin/env node
// WorkBuddy Shift: 悬浮窗公开演示的 postMessage 契约检查（无第三方依赖）。
//
// 主界面（src/lib/companion-demo-script.ts）与 companion-ui 演示桥
// （companion-ui/src/demo/protocol.ts）必须使用同一个 channel；历史上两边各改一半
// 导致演示永久停在“连接中”。本脚本在 CI 里兜底，避免再次分叉。
//
// 用法：node scripts/check-companion-contract.mjs

import fs from "node:fs";
import path from "node:path";

const root = path.resolve(import.meta.dirname, "..");
const read = (rel) => fs.readFileSync(path.join(root, rel), "utf8");

function extract(file, pattern, label) {
  const match = read(file).match(pattern);
  if (!match) {
    console.error("check-companion-contract: FAIL");
    console.error("  - " + label + " 里找不到 channel 常量: " + file);
    process.exit(1);
  }
  return match[1];
}

const appChannel = extract(
  "src/lib/companion-demo-script.ts",
  /COMPANION_DEMO_CHANNEL\s*=\s*"([^"]+)"/,
  "主界面",
);
const railChannel = extract(
  "companion-ui/src/demo/protocol.ts",
  /DEMO_CHANNEL\s*=\s*'([^']+)'/,
  "companion-ui 演示桥",
);

if (appChannel !== railChannel) {
  console.error("check-companion-contract: FAIL");
  console.error("  - channel 不一致：主界面 \"" + appChannel + "\"，iframe \"" + railChannel + "\"");
  console.error("  - 两处必须同时修改（src/lib/companion-demo-script.ts 与 companion-ui/src/demo/protocol.ts）");
  process.exit(1);
}

console.log("check-companion-contract: PASS (" + appChannel + ")");

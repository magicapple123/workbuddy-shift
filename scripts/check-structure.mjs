#!/usr/bin/env node
// 结构约束门禁（AGENTS.md 第 4/5/7/9 节里可机械验证的部分）。
//
// 这些规则以前只写在文档里，靠评审记忆执行；一旦有人（或 AI）改了结构却忘了同步文档，
// 要等到很久以后才会被发现。这里把它们变成 CI 失败：
//
//   1. 文件规模：>500 行手写源码必须在 docs/ARCHITECTURE.md 的豁免表登记，且行数一致
//      （双向校验：未登记 / 行数不符 / 已缩小但条目未删，都会失败）
//   2. 依赖方向：wb-shift-core 不得依赖 tauri / axum，源码不得出现 tauri:: / axum::
//   3. 旧产品名：派生来源的三种写法不得回流到代码、配置、文档
//      （仅 NOTICE / LICENSE / README / AGENTS 的策略与归属声明豁免）
//   4. Tauri capability：不得出现 fs / shell / process 这类宽泛权限
//   5. lint 抑制棘轮：#[allow(unused_imports)] 不得超过基线（跨平台 cfg 的既有债务），
//      新增即失败；清理后把 BASELINE 下调
//
// 用法：node scripts/check-structure.mjs
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const problems = [];

const SOURCE_EXTS = new Set([".rs", ".ts", ".tsx", ".js", ".jsx", ".mjs"]);
const TEXT_EXTS = new Set([
  ...SOURCE_EXTS,
  ".json",
  ".yml",
  ".yaml",
  ".toml",
  ".md",
  ".sh",
  ".ps1",
  ".cmd",
  ".html",
  ".css",
]);
const SOURCE_DIRS = ["crates", "src-tauri/src", "src", "companion-ui/src", "scripts"];
const SKIP_DIRS = new Set([
  "node_modules",
  "target",
  "dist",
  "dist-embed",
  "dist-demo",
  ".git",
  "public",
  "binaries",
]);
const LEGACY_FILES = new Set(["NOTICE", "LICENSE", "README.md", "AGENTS.md"]);
// 跨平台 cfg 与测试可见性需要的既有抑制数量；只允许下降，不允许上升。
// 2026-11-16 第一轮按模块清理：180 → 91（详见 docs/ARCHITECTURE.md 的类级登记）。
const ALLOW_UNUSED_IMPORTS_BASELINE = 91;

const toPosix = (file) => path.relative(ROOT, file).split(path.sep).join("/");

function walk(dir, out = []) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    if (entry.isDirectory()) {
      if (SKIP_DIRS.has(entry.name)) continue;
      walk(path.join(dir, entry.name), out);
    } else {
      out.push(path.join(dir, entry.name));
    }
  }
  return out;
}

function lineCount(file) {
  // 与 `wc -l` 同口径：数换行符。
  return fs.readFileSync(file, "utf8").split("\n").length - 1;
}

// ---------------------------------------------------------------------------
// 1) >500 行文件 ↔ 豁免表
// ---------------------------------------------------------------------------
const docRel = "docs/ARCHITECTURE.md";
const docAbs = path.join(ROOT, docRel);
let documented = new Map();
if (!fs.existsSync(docAbs)) {
  problems.push(`${docRel}: 文件不存在，无法校验长文件豁免表`);
} else {
  const text = fs.readFileSync(docAbs, "utf8");
  const heading = "### 仍未拆分的 >500 行文件";
  const start = text.indexOf(heading);
  if (start < 0) {
    problems.push(`${docRel}: 未找到「${heading}」小节`);
  } else {
    const section = text.slice(start).split(/\n### /)[0];
    for (const match of section.matchAll(/^\|\s*`([^`]+)`\s*\|\s*(\d+)\s*\|/gm)) {
      const [, rel, lines] = match;
      if (documented.has(rel)) problems.push(`${docRel}: 豁免表重复登记 ${rel}`);
      documented.set(rel, Number(lines));
    }
  }
}

const sourceFiles = [];
for (const dir of SOURCE_DIRS) {
  const abs = path.join(ROOT, dir);
  if (fs.existsSync(abs)) walk(abs, sourceFiles);
}
const handWritten = sourceFiles.filter((file) => SOURCE_EXTS.has(path.extname(file)));

let oversizeCount = 0;
for (const file of handWritten) {
  const rel = toPosix(file);
  const lines = lineCount(file);
  const registered = documented.get(rel);
  if (lines > 500) oversizeCount += 1;
  if (registered !== undefined) {
    if (lines !== registered) {
      problems.push(`${rel}: 实际 ${lines} 行，豁免表登记 ${registered} 行`);
    }
    if (lines <= 500) {
      problems.push(`${rel}: 已降到 ${lines} 行（≤500），豁免表条目应删除`);
    }
  } else if (lines > 500) {
    problems.push(`${rel}: ${lines} 行（>500）未在 ${docRel} 豁免表登记`);
  }
}
for (const rel of documented.keys()) {
  if (!fs.existsSync(path.join(ROOT, rel))) {
    problems.push(`${docRel}: 豁免表条目指向不存在的文件 ${rel}`);
  }
}

// ---------------------------------------------------------------------------
// 2) 依赖方向：core 不依赖宿主框架
// ---------------------------------------------------------------------------
function dependencyNames(tomlText) {
  const names = [];
  let section = "";
  for (const rawLine of tomlText.split(/\r?\n/)) {
    const line = rawLine.trim();
    const header = line.match(/^\[([^\]]+)\]$/);
    if (header) {
      section = header[1];
      continue;
    }
    if (!/(^|\.)dependencies$/.test(section)) continue;
    const dep = line.match(/^([A-Za-z0-9_-]+)\s*=/);
    if (dep) names.push(dep[1]);
  }
  return names;
}

const coreToml = path.join(ROOT, "crates/wb-shift-core/Cargo.toml");
if (!fs.existsSync(coreToml)) {
  problems.push("crates/wb-shift-core/Cargo.toml: 文件不存在");
} else {
  for (const dep of dependencyNames(fs.readFileSync(coreToml, "utf8"))) {
    if (/^(tauri|axum)(-|$)/.test(dep)) {
      problems.push(`wb-shift-core 不得依赖 ${dep}（core 必须与宿主框架解耦）`);
    }
  }
}
const coreSources = fs.existsSync(path.join(ROOT, "crates/wb-shift-core/src"))
  ? walk(path.join(ROOT, "crates/wb-shift-core/src")).filter((f) => f.endsWith(".rs"))
  : [];
for (const file of coreSources) {
  const match = fs.readFileSync(file, "utf8").match(/\b(tauri|axum)::/);
  if (match) {
    problems.push(`${toPosix(file)}: 出现 ${match[1]}::（core 不得引用宿主框架）`);
  }
}

// ---------------------------------------------------------------------------
// 3) 旧产品名不得回流
// ---------------------------------------------------------------------------
const LEGACY_PATTERNS = [
  new RegExp("workbuddy[-_ ]switch", "i"),
  new RegExp("\\bwb[-_]switch\\b", "i"),
];
// AGENTS.md 第 7 节的「已知并允许的旧名称例外」，按文件登记（不是按目录、不是关掉检查）。
// 每一项都必须写清：理由 / 影响 / 后续计划（对应第 9 节的四项要求）。
const LEGACY_NAME_EXCEPTIONS = new Map([
  [
    "crates/wb-shift-core/src/modules/rate_limit_hook/mod.rs",
    {
      reason:
        "识别上一代数据目录，清理它遗留的 hook 脚本与注册条目（不认出就会双触发、事件写进废弃目录）",
      impact: "只读不删旧目录；命中即把条目换成本工具当前形态，第三方 hook.sh 不受影响",
      plan: "旧目录在用户机上消失（或迁移工具接管）后删除该常量和这条登记",
    },
  ],
]);
const legacyFiles = [];
for (const dir of ["crates", "src-tauri", "src", "companion-ui/src", "scripts", "docs", "npm"]) {
  const abs = path.join(ROOT, dir);
  if (fs.existsSync(abs)) walk(abs, legacyFiles);
}
legacyFiles.push(path.join(ROOT, "README.md"), path.join(ROOT, "AGENTS.md"));
for (const file of legacyFiles) {
  if (!fs.existsSync(file) || !TEXT_EXTS.has(path.extname(file))) continue;
  const rel = toPosix(file);
  if (LEGACY_FILES.has(rel)) continue;
  if (LEGACY_NAME_EXCEPTIONS.has(rel)) continue;
  if (rel === "pnpm-lock.yaml" || rel.endsWith("package-lock.json")) continue;
  const text = fs.readFileSync(file, "utf8");
  for (const pattern of LEGACY_PATTERNS) {
    const match = text.match(pattern);
    if (match) {
      problems.push(`${rel}: 出现旧产品名 "${match[0]}"（仅 NOTICE/LICENSE/README 可保留归属声明）`);
      break;
    }
  }
}

// ---------------------------------------------------------------------------
// 4) Tauri capability 最小化
// ---------------------------------------------------------------------------
const capabilityDir = path.join(ROOT, "src-tauri/capabilities");
if (fs.existsSync(capabilityDir)) {
  for (const name of fs.readdirSync(capabilityDir).filter((file) => file.endsWith(".json"))) {
    const rel = `src-tauri/capabilities/${name}`;
    let data;
    try {
      data = JSON.parse(fs.readFileSync(path.join(capabilityDir, name), "utf8"));
    } catch (error) {
      problems.push(`${rel}: JSON 解析失败（${error.message}）`);
      continue;
    }
    for (const permission of data.permissions ?? []) {
      const id = typeof permission === "string" ? permission : permission?.identifier;
      if (typeof id === "string" && /^(fs|shell|process):/.test(id)) {
        problems.push(`${rel}: 宽泛权限 ${id}（AGENTS 第 5 节禁止，确需时先论证 scope）`);
      }
    }
  }
}

// ---------------------------------------------------------------------------
// 5) 前端分层：pages / components 不得直接 import Tauri 插件
// ---------------------------------------------------------------------------
const directTauri = /from\s+["']@tauri-apps\/|import\(\s*["']@tauri-apps\//;
const stripComments = (text) =>
  text.replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "");
for (const dir of ["src/pages", "src/components"]) {
  const abs = path.join(ROOT, dir);
  if (!fs.existsSync(abs)) continue;
  for (const file of walk(abs).filter((f) => f.endsWith(".ts") || f.endsWith(".tsx"))) {
    if (directTauri.test(stripComments(fs.readFileSync(file, "utf8")))) {
      problems.push(
        `${toPosix(file)}: 直接 import @tauri-apps/*（页面/组件只能经 @/lib/api 的 desktop 包装）`,
      );
    }
  }
}

// ---------------------------------------------------------------------------
// 6) lint 抑制棘轮
// ---------------------------------------------------------------------------
let allowUnusedImports = 0;
for (const file of sourceFiles.filter((f) => f.endsWith(".rs"))) {
  const text = fs.readFileSync(file, "utf8");
  allowUnusedImports += (text.match(/#\[allow\(unused_imports\)\]/g) ?? []).length;
}
if (allowUnusedImports > ALLOW_UNUSED_IMPORTS_BASELINE) {
  problems.push(
    `#[allow(unused_imports)] 共 ${allowUnusedImports} 处，超过基线 ${ALLOW_UNUSED_IMPORTS_BASELINE}：` +
      "新增抑制必须写理由并登记（AGENTS 第 9 节），或改为显式导入",
  );
}

if (problems.length > 0) {
  console.error(`check-structure: 发现 ${problems.length} 处结构偏差`);
  for (const problem of problems) console.error(`  - ${problem}`);
  process.exit(1);
}

const allowNote =
  allowUnusedImports < ALLOW_UNUSED_IMPORTS_BASELINE
    ? `，已低于基线（可把 BASELINE 下调到 ${allowUnusedImports}）`
    : "";
const legacyNote =
  LEGACY_NAME_EXCEPTIONS.size > 0
    ? `，已登记例外 ${LEGACY_NAME_EXCEPTIONS.size} 处（见 AGENTS.md 第 7 节）`
    : "";
console.log(
  `check-structure: PASS（>500 行文件 ${oversizeCount} 个与豁免表一致；core 依赖方向 OK；` +
    `旧名称 0 处${legacyNote}；capability 最小化 OK；allow(unused_imports) ${allowUnusedImports}/${ALLOW_UNUSED_IMPORTS_BASELINE}${allowNote}）`,
);

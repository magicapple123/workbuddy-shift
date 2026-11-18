#!/usr/bin/env node
// 版本一致性校验（纯 Node、无第三方依赖）：唯一版本源是根 Cargo.toml 的
// [workspace.package] version，其余位置必须全部与之相同：
//   - package.json / src-tauri/tauri.conf.json / npm/package.json / companion-ui/package.json
//   - npm/platform/*/package.json（每个平台包）
//   - npm/package.json 的 optionalDependencies（主包引用平台包的版本）
//   - Cargo.lock 里所有 workspace 成员（\[\[package\]\] 条目）的 version
// 用法：node scripts/check-versions.mjs
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const problems = [];

function readFile(relPath) {
  try {
    return fs.readFileSync(path.join(ROOT, relPath), "utf8");
  } catch (err) {
    problems.push(`${relPath}: 读取失败（${err.code ?? err.message}）`);
    return null;
  }
}

function readJson(relPath) {
  const text = readFile(relPath);
  if (text === null) return null;
  try {
    return JSON.parse(text);
  } catch (err) {
    problems.push(`${relPath}: JSON 解析失败（${err.message}）`);
    return null;
  }
}

// 取指定 TOML 段落里的字符串键。仓库内 manifest 格式固定（[section] + key = "value"），
// 不引入 TOML 解析依赖；只读 section 内第一个匹配的键。
function sectionString(relPath, section, key) {
  const text = readFile(relPath);
  if (text === null) return null;
  let current = "";
  for (const rawLine of text.split(/\r?\n/)) {
    const line = rawLine.trim();
    const header = line.match(/^\[([^\]]+)\]/);
    if (header) {
      current = header[1].trim();
      continue;
    }
    if (current !== section) continue;
    const value = line.match(new RegExp(`^${key}\\s*=\\s*"([^"]*)"`));
    if (value) return value[1];
  }
  problems.push(`${relPath}: 未找到 [${section}] ${key}`);
  return null;
}

// 根 Cargo.toml 的 [workspace] members 段落文本（members 数组可能跨多行）
function workspaceSectionText() {
  const text = readFile("Cargo.toml");
  if (text === null) return "";
  // 兼容 Windows checkout（crlf）
  const match = text.match(/^\[workspace\][ \t]*\r?$/m);
  if (!match) {
    problems.push("Cargo.toml: 未找到 [workspace] 段落");
    return "";
  }
  const rest = text.slice(match.index);
  const nextHeader = rest.slice(1).search(/^\[/m);
  return nextHeader < 0 ? rest : rest.slice(0, nextHeader + 1);
}

function workspaceMemberNames() {
  const workspaceText = workspaceSectionText();
  const members = workspaceText.match(/members\s*=\s*\[([\s\S]*?)\]/);
  if (!members) {
    problems.push("Cargo.toml: 未找到 [workspace] members 数组");
    return [];
  }
  const names = [];
  for (const member of members[1].matchAll(/"([^"]+)"/g)) {
    const relPath = path.posix.join(member[1].replace(/\\/g, "/"), "Cargo.toml");
    const name = sectionString(relPath, "package", "name");
    if (name) names.push(name);
  }
  return names;
}

// Cargo.lock 的 [[package]] 条目：name -> [version, ...]
function cargoLockVersions() {
  const text = readFile("Cargo.lock");
  const versions = new Map();
  if (text === null) return versions;
  for (const block of text.split(/^\[\[package\]\][ \t]*\r?$/m)) {
    const name = block.match(/^name\s*=\s*"([^"]+)"/m);
    const version = block.match(/^version\s*=\s*"([^"]+)"/m);
    if (!name || !version) continue;
    const list = versions.get(name[1]) ?? [];
    list.push(version[1]);
    versions.set(name[1], list);
  }
  return versions;
}

let expected = null;
let workspaceMemberCount = 0;

function expect(label, actual) {
  if (actual === null || actual === undefined) return;
  if (actual !== expected) problems.push(`${label}: ${actual}（期望 ${expected}）`);
}

/**
 * Tauri 的 JS 包与 Rust crate 必须同 major/minor：`tauri build` 在打包前会校验，
 * 不一致直接报 "Found version mismatched Tauri packages" 并退出。
 *
 * 2026-11 重建锁文件时，`@tauri-apps/api: ^2` 被解析到 2.12.4，而 Cargo.lock 里
 * tauri 仍是 2.11.5 —— typecheck/test 全绿，直到整包构建才暴露。所以要求写成完整
 * 版本（不给 caret 漂移空间），并在每次改依赖时就地校验。
 */
function checkTauriPairing(lockVersions) {
  const rootPkg = readJson("package.json");
  if (!rootPkg) return;
  for (const [name, spec] of Object.entries(rootPkg.dependencies ?? {})) {
    let crate = null;
    if (name === "@tauri-apps/api") crate = "tauri";
    else if (name.startsWith("@tauri-apps/plugin-")) {
      crate = `tauri-plugin-${name.slice("@tauri-apps/plugin-".length)}`;
    }
    if (!crate) continue;

    const crateVersions = lockVersions.get(crate);
    if (!crateVersions || crateVersions.length !== 1) {
      problems.push(`Cargo.lock: 未找到唯一的 ${crate}（${name} 的 Rust 侧）`);
      continue;
    }
    const match = String(spec).match(/^(\d+)\.(\d+)\.(\d+)/);
    if (!match) {
      problems.push(
        `${name}: 必须写成完整版本 x.y.z（当前 ${spec}），才能与 ${crate} ${crateVersions[0]} 校验`,
      );
      continue;
    }
    const jsMinor = `${match[1]}.${match[2]}`;
    const crateMinor = crateVersions[0].split(".").slice(0, 2).join(".");
    if (jsMinor !== crateMinor) {
      problems.push(
        `${name} ${spec} 与 ${crate} ${crateVersions[0]} 的 minor 不一致（tauri build 会拒绝；升级时两侧一起改）`,
      );
    }
  }
}

function checkAll() {
  expected = sectionString("Cargo.toml", "workspace.package", "version");
  if (expected === null) return;

  expect("package.json version", readJson("package.json")?.version);
  expect("src-tauri/tauri.conf.json version", readJson("src-tauri/tauri.conf.json")?.version);

  const npmPkg = readJson("npm/package.json");
  if (npmPkg) {
    expect("npm/package.json version", npmPkg.version);
    for (const [name, version] of Object.entries(npmPkg.optionalDependencies ?? {})) {
      expect(`npm/package.json optionalDependencies["${name}"]`, version);
    }
  }

  expect("companion-ui/package.json version", readJson("companion-ui/package.json")?.version);

  const platformRoot = path.join(ROOT, "npm", "platform");
  let platformDirs = [];
  try {
    platformDirs = fs
      .readdirSync(platformRoot, { withFileTypes: true })
      .filter((entry) => entry.isDirectory())
      .map((entry) => entry.name)
      .sort();
  } catch (err) {
    problems.push(`npm/platform: 读取失败（${err.code ?? err.message}）`);
  }
  for (const dir of platformDirs) {
    const relPath = `npm/platform/${dir}/package.json`;
    expect(`${relPath} version`, readJson(relPath)?.version);
  }

  const memberNames = workspaceMemberNames();
  workspaceMemberCount = memberNames.length;
  const lockVersions = cargoLockVersions();
  if (memberNames.length === 0) {
    problems.push("Cargo.toml: workspace 成员列表为空，无法校验 Cargo.lock");
  }
  for (const name of memberNames) {
    const found = lockVersions.get(name);
    if (!found) {
      problems.push(`Cargo.lock: 未找到 workspace 成员 "${name}" 的 [[package]] 条目`);
    } else if (found.length !== 1) {
      problems.push(`Cargo.lock: workspace 成员 "${name}" 出现 ${found.length} 次（期望 1 次）`);
    } else {
      expect(`Cargo.lock [[package]] "${name}" version`, found[0]);
    }
  }

  checkTauriPairing(lockVersions);
}

// 包管理器唯一性：pnpm 是唯一入口（决策与理由见 docs/DEVELOPMENT.md 的「包管理器」节）。
// 只检测锁文件、不自动修复：出现即说明有人用别的包管理器装过，需要先决定是否迁移。
// pnpm 本身必须留下 lockfile，否则 CI 的 --frozen-lockfile 无从校验依赖树。
if (!fs.existsSync(path.join(ROOT, "pnpm-lock.yaml"))) {
  problems.push("pnpm-lock.yaml: 缺失（仓库用 pnpm 管理前端依赖，锁文件必须提交）");
}
for (const stray of ["package-lock.json", "yarn.lock", "bun.lockb", "bun.lock"]) {
  if (fs.existsSync(path.join(ROOT, stray))) {
    problems.push(
      `${stray}: 仓库已改用 pnpm，不得保留其它包管理器的锁文件（见 docs/DEVELOPMENT.md「包管理器」节）`,
    );
  }
}
// companion-ui 是 workspace 成员，不能有自己的锁文件/独立安装。
for (const stray of ["companion-ui/package-lock.json", "companion-ui/pnpm-lock.yaml"]) {
  if (fs.existsSync(path.join(ROOT, stray))) {
    problems.push(`${stray}: companion-ui 由根 pnpm workspace 统一安装，不得单独加锁文件`);
  }
}

checkAll();

if (problems.length > 0) {
  console.error(
    `check-versions: 发现 ${problems.length} 处问题（版本唯一来源：Cargo.toml [workspace.package] version；包管理器只用 pnpm）`,
  );
  for (const problem of problems) console.error(`  - ${problem}`);
  process.exit(1);
}

console.log(`check-versions: 版本一致（${expected}）`);
console.log(
  `  校验来源：Cargo.toml [workspace.package]、package.json、src-tauri/tauri.conf.json、npm/package.json（含 optionalDependencies）、npm/platform/*、companion-ui/package.json、Cargo.lock（${workspaceMemberCount} 个 workspace 成员）`,
);

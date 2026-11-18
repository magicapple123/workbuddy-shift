#!/usr/bin/env node
// WorkBuddy Shift: 生产 Rust 代码禁止「加锁后 unwrap/expect」（锁中毒必须可处理）。
//
// 检查范围：crates/**/src/**/*.rs 与 src-tauri/src/**/*.rs。
// 豁免：`tests/` 目录、文件名 `tests.rs`、以及 `#[cfg(test)]` 项内部（含单元测试模块）。
//
// 判定方式：找到 `.lock()` / `.read()` / `.write()`（无参数调用），再看同一条语句
// （到下一个括号深度为 0 的 `;`）里是否出现 `.unwrap(` / `.expect(`。这样
// `let g = m.lock().map_err(...)?;` 不会误报，`m.lock()\n.unwrap().iter()` 会命中。
//
// 为什么不用 clippy：`clippy::unwrap_used` 无法只针对锁且会命中大量无害的
// `Option::unwrap`；而锁中毒是能靠调用形状精确定位的规则。
//
// 用法：node scripts/check-rust-locks.mjs
// 退出码：0 通过；1 命中。

import fs from "node:fs";
import path from "node:path";

const root = path.resolve(import.meta.dirname, "..");
const ROOTS = ["crates", "src-tauri/src"];

function rustFilesUnder(rel) {
  const abs = path.join(root, rel);
  if (!fs.existsSync(abs)) return [];
  const stat = fs.statSync(abs);
  if (stat.isFile()) return rel.endsWith(".rs") ? [rel] : [];
  const out = [];
  for (const entry of fs.readdirSync(abs, { withFileTypes: true })) {
    const child = path.join(rel, entry.name);
    if (entry.isDirectory()) out.push(...rustFilesUnder(child));
    else if (entry.name.endsWith(".rs")) out.push(child);
  }
  return out;
}

/**
 * 返回「位于 #[cfg(test)] 项内部」的字符区间。
 *
 * 只处理与我们相关的语法：行/块注释、普通/原始/字节字符串、字符字面量与生命周期。
 * 目的是把花括号配对算对，不需要完整的 Rust 词法分析。
 */
function cfgTestRanges(text) {
  const braces = [];
  const cfgMarkers = [];
  let i = 0;
  const n = text.length;
  while (i < n) {
    const ch = text[i];
    const next = text[i + 1];
    if (ch === "/" && next === "/") {
      i = text.indexOf("\n", i);
      if (i < 0) break;
      continue;
    }
    if (ch === "/" && next === "*") {
      const end = text.indexOf("*/", i + 2);
      i = end < 0 ? n : end + 2;
      continue;
    }
    // 原始字符串：r"..." / r#"..."# / br#"..."#
    if ((ch === "r" || (ch === "b" && next === "r")) && /[#"]/.test(text[i + (ch === "r" ? 1 : 2)] ?? "")) {
      let cursor = i + (ch === "r" ? 1 : 2);
      let hashes = 0;
      while (text[cursor] === "#") {
        hashes += 1;
        cursor += 1;
      }
      if (text[cursor] === '"') {
        const closing = '"' + "#".repeat(hashes);
        const end = text.indexOf(closing, cursor + 1);
        i = end < 0 ? n : end + closing.length;
        continue;
      }
    }
    if (ch === '"' || (ch === "b" && next === '"')) {
      let cursor = ch === '"' ? i + 1 : i + 2;
      while (cursor < n) {
        if (text[cursor] === "\\") {
          cursor += 2;
          continue;
        }
        if (text[cursor] === '"') break;
        cursor += 1;
      }
      i = cursor + 1;
      continue;
    }
    if (ch === "'" || (ch === "b" && next === "'")) {
      const quote = ch === "'" ? i : i + 1;
      // 区分字符字面量（'x' / '\n'）与生命周期（'a）。
      const isEscaped = text[quote + 1] === "\\";
      const closesImmediately = text[quote + 2] === "'";
      if (isEscaped || closesImmediately) {
        let cursor = quote + 1;
        while (cursor < n) {
          if (text[cursor] === "\\") {
            cursor += 2;
            continue;
          }
          if (text[cursor] === "'") break;
          cursor += 1;
        }
        i = cursor + 1;
        continue;
      }
      i = quote + 1;
      continue;
    }
    if (ch === "#" && text.startsWith("#[cfg(test)]", i)) {
      cfgMarkers.push(i);
      i += "#[cfg(test)]".length;
      continue;
    }
    if (ch === "{") braces.push(i);
    else if (ch === "}") braces.push(-i);
    i += 1;
  }

  // 用带符号的栈做配对：正数 = 开括号位置，负数 = 闭括号位置。
  const ranges = [];
  const openIndex = new Map();
  const stack = [];
  for (const item of braces) {
    if (item >= 0) {
      stack.push(item);
    } else {
      const open = stack.pop();
      if (open !== undefined) openIndex.set(open, -item);
    }
  }
  for (const marker of cfgMarkers) {
    const open = braces.find((item) => item >= 0 && item > marker);
    if (open === undefined || !openIndex.has(open)) continue;
    ranges.push([marker, openIndex.get(open)]);
  }
  return ranges;
}

/** 从 pos 起找到同一条语句的结尾（括号深度 0 的 `;`），返回区间文本。 */
function statementAt(text, pos) {
  let depth = 0;
  for (let i = pos; i < text.length; i += 1) {
    const ch = text[i];
    if (ch === "(" || ch === "[" || ch === "{") depth += 1;
    else if (ch === ")" || ch === "]" || ch === "}") depth -= 1;
    else if (ch === ";" && depth <= 0) return text.slice(pos, i + 1);
  }
  return text.slice(pos, Math.min(text.length, pos + 400));
}

const failures = [];
const scanned = [];

for (const relRoot of ROOTS) {
  for (const rel of rustFilesUnder(relRoot)) {
    const posix = rel.split(path.sep).join("/");
    if (posix.includes("/tests/") || path.basename(posix) === "tests.rs") continue;
    const text = fs.readFileSync(path.join(root, rel), "utf8");
    const exempt = cfgTestRanges(text);
    const inTest = (index) => exempt.some(([start, end]) => index >= start && index <= end);
    scanned.push(posix);

    const pattern = /\.(?:lock|read|write)\(\)/g;
    let match = pattern.exec(text);
    while (match) {
      const statement = statementAt(text, match.index);
      if (/\.(?:unwrap|expect)\(/.test(statement) && !inTest(match.index)) {
        const line = text.slice(0, match.index).split("\n").length;
        failures.push(
          `${posix}:${line}: ${statement.split("\n")[0].trim()} … 锁中毒必须返回错误或用 locks::recover（并注明理由）`,
        );
      }
      match = pattern.exec(text);
    }
  }
}

if (failures.length > 0) {
  console.error(`check-rust-locks: FAIL（${failures.length} 处）`);
  for (const failure of failures) console.error("  - " + failure);
  process.exit(1);
}
console.log(`check-rust-locks: PASS（扫描 ${scanned.length} 个生产 Rust 文件）`);

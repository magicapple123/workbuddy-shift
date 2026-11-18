#!/usr/bin/env node
// WorkBuddy Shift: HTTP ↔ Tauri 请求体契约检查（无第三方依赖）。
//
// 现有 check-command-routes.mjs 只校验「命令名 / method / path / handler」四方对应，
// 不校验请求体形状。这个脚本补上第二部分：
//
//   1. POST handler 的 body 必须是明确 DTO（`Json<SomeRequest>` / `Result<Json<..>>`）；
//      仍是 `Json<Value>` 的必须登记在 scripts/api-contract-baseline.json（含理由，
//      且 handler 不存在或已迁移时会报「过期条目」）。
//   2. 有 DTO 的 POST 命令，DTO 字段（camelCase）必须与 Tauri command 参数逐字一致
//      （忽略 app/window/state 这类宿主参数）。两侧不一致 = WebUI 与桌面端接受
//      不同的请求体，正是要拦的漂移。
//
// 不校验响应体：两侧都直接返回 core 的 serde_json::Value，形状同源；前端契约在
// src/lib/types/，由 TypeScript 编译与 Vitest 覆盖。
//
// 用法：node scripts/check-api-contract.mjs
// 退出码：0 通过；1 有缺失/漂移/过期登记。

import fs from "node:fs";
import path from "node:path";

const root = path.resolve(import.meta.dirname, "..");
const read = (rel) => fs.readFileSync(path.join(root, rel), "utf8");
const failures = [];
const info = [];

/** 取 `source` 中 `fn <name>` 之后的括号参数列表文本。 */
function fnParams(source, name) {
  const pattern = new RegExp(`fn\\s+${name}\\s*\\(`);
  const match = pattern.exec(source);
  if (!match) return null;
  const start = match.index + match[0].length - 1;
  let depth = 0;
  for (let i = start; i < source.length; i += 1) {
    if (source[i] === "(") depth += 1;
    else if (source[i] === ")") {
      depth -= 1;
      if (depth === 0) return source.slice(start + 1, i);
    }
  }
  return null;
}

/** 按顶层逗号切分（忽略 <> () [] 内的逗号）。 */
function splitTopLevel(text, separator = ",") {
  const out = [];
  let depth = 0;
  let current = "";
  for (const ch of text) {
    if ("<([".includes(ch)) depth += 1;
    else if (">)]".includes(ch)) depth -= 1;
    if (ch === separator && depth === 0) {
      out.push(current);
      current = "";
      continue;
    }
    current += ch;
  }
  if (current.trim()) out.push(current);
  return out.map((item) => item.trim()).filter(Boolean);
}

function snakeToCamel(name) {
  return name.replace(/_([a-z0-9])/g, (_, ch) => ch.toUpperCase());
}

function rustFilesUnder(rel) {
  const abs = path.join(root, rel);
  if (!fs.existsSync(abs)) return [];
  const out = [];
  for (const entry of fs.readdirSync(abs, { withFileTypes: true })) {
    const child = path.join(rel, entry.name);
    if (entry.isDirectory()) out.push(...rustFilesUnder(child));
    else if (entry.name.endsWith(".rs")) out.push(child);
  }
  return out;
}

/** 解析 `pub(crate) struct X { ... }` 的字段名（含 rename_all 处理）。 */
function dtoFields(source, structName) {
  const header = new RegExp(`struct\\s+${structName}\\b`, "g");
  const match = header.exec(source);
  if (!match) return null;
  const before = source.slice(Math.max(0, match.index - 240), match.index);
  const camel = /rename_all\s*=\s*"camelCase"/.test(before);
  const open = source.indexOf("{", match.index);
  if (open < 0) return null;
  let depth = 0;
  let end = -1;
  for (let i = open; i < source.length; i += 1) {
    if (source[i] === "{") depth += 1;
    else if (source[i] === "}") {
      depth -= 1;
      if (depth === 0) {
        end = i;
        break;
      }
    }
  }
  if (end < 0) return null;
  const body = source.slice(open + 1, end);
  const fields = [];
  for (const line of body.split(/\r?\n/)) {
    const field = line.match(/^\s*pub(?:\([^)]*\))?\s+([a-z0-9_]+)\s*:/);
    if (field) fields.push(camel ? snakeToCamel(field[1]) : field[1]);
  }
  return fields;
}

// ---------------------------------------------------------------------------
// 1) 前端 ROUTES：command -> { method, path }
// ---------------------------------------------------------------------------
const coreTs = read("src/lib/api/core.ts");
const routesBlock = coreTs.match(/const ROUTES: Record<string, Route> = \{([\s\S]*?)\n\};/);
if (!routesBlock) {
  console.error("check-api-contract: 找不到 core.ts 的 ROUTES 表");
  process.exit(1);
}
const routes = new Map();
for (const match of routesBlock[1].matchAll(
  /([a-zA-Z0-9_]+)\s*:\s*\{\s*method:\s*"(GET|POST)"\s*,\s*path:\s*"([^"]+)"\s*,?\s*\}/g,
)) {
  routes.set(match[1], { method: match[2], path: match[3] });
}

// ---------------------------------------------------------------------------
// 2) server router：`METHOD path` -> handler
//    一个 .route() 可以链式挂多个方法（get(a).post(b)），必须解析整段表达式。
// ---------------------------------------------------------------------------
const routerSource = read("crates/wb-shift-server/src/api/router.rs").replace(/\s+/g, " ");
const routeByPath = new Map();
let cursor = 0;
while ((cursor = routerSource.indexOf(".route(", cursor)) !== -1) {
  const start = cursor + ".route(".length;
  let depth = 1;
  let index = start;
  while (index < routerSource.length && depth > 0) {
    if (routerSource[index] === "(") depth += 1;
    else if (routerSource[index] === ")") depth -= 1;
    index += 1;
  }
  const expression = routerSource.slice(start, index - 1);
  const pathMatch = expression.match(/^\s*"([^"]+)"/);
  if (pathMatch) {
    for (const method of expression.matchAll(/\b(get|post)\(\s*(\w+)\)/g)) {
      routeByPath.set(`${method[1].toUpperCase()} ${pathMatch[1]}`, method[2]);
    }
  }
  cursor = index;
}

// ---------------------------------------------------------------------------
// 3) handler 源文件
// ---------------------------------------------------------------------------
const handlersSource = rustFilesUnder("crates/wb-shift-server/src/api/handlers")
  .map(read)
  .join("\n");
const handlerNames = new Set(
  [...handlersSource.matchAll(/pub\(crate\)\s+(?:async\s+)?fn\s+(\w+)/g)].map((m) => m[1]),
);

function bodyKind(handler) {
  const params = fnParams(handlersSource, handler);
  if (params === null) return { kind: "missing" };
  const json = params.match(/Json<([A-Za-z0-9_]+)>/);
  if (json) return { kind: json[1] === "Value" ? "raw" : "typed", type: json[1] };
  return { kind: "none" };
}

// ---------------------------------------------------------------------------
// 4) Tauri command 参数
// ---------------------------------------------------------------------------
const HOST_PARAMS = new Set(["app", "window", "state"]);
const commandParams = new Map();
for (const rel of rustFilesUnder("src-tauri/src/commands")) {
  const source = read(rel);
  for (const match of source.matchAll(
    /#\[tauri::command(?:\([^)]*\))?\]\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+(\w+)/g,
  )) {
    const name = match[1];
    const params = fnParams(source, name);
    if (params === null) continue;
    const names = [];
    for (const param of splitTopLevel(params)) {
      const parsed = param.match(/^(_?[a-z0-9_]+)\s*:/);
      if (!parsed) continue;
      const raw = parsed[1].replace(/^_/, "");
      if (HOST_PARAMS.has(raw)) continue;
      names.push(snakeToCamel(raw));
    }
    commandParams.set(name, names);
  }
}

// ---------------------------------------------------------------------------
// 5) DTO 字段
// ---------------------------------------------------------------------------
const dtoSource = read("crates/wb-shift-server/src/api/dto.rs");

// ---------------------------------------------------------------------------
// 6) baseline（未迁移的 Json<Value> handler + 理由）
// ---------------------------------------------------------------------------
const baselineRel = "scripts/api-contract-baseline.json";
let baseline = {};
if (fs.existsSync(path.join(root, baselineRel))) {
  try {
    baseline = JSON.parse(read(baselineRel)).rawBodyHandlers ?? {};
  } catch (error) {
    failures.push(`${baselineRel} 不是合法 JSON：${error.message}`);
  }
} else {
  failures.push(`缺少 ${baselineRel}`);
}

// ---------------------------------------------------------------------------
// 7) 检查
// ---------------------------------------------------------------------------
const checkedCommands = new Set();
const rawHandlers = new Set();

for (const [command, route] of routes) {
  if (route.method !== "POST") continue;
  const handler = routeByPath.get(`POST ${route.path}`);
  if (!handler) {
    failures.push(`ROUTES 路径没有 server handler: POST ${route.path} (command ${command})`);
    continue;
  }
  const body = bodyKind(handler);
  if (body.kind === "missing") {
    failures.push(`在 handlers 里找不到 ${handler}`);
    continue;
  }
  if (body.kind === "raw") {
    rawHandlers.add(handler);
    if (!baseline[handler]) {
      failures.push(
        `${handler}（POST ${route.path}）仍用 Json<Value>：迁移为 DTO，或在 ${baselineRel} 登记理由`,
      );
    }
    continue;
  }
  if (body.kind !== "typed") continue;

  // 已迁移：DTO 字段必须与 Tauri 命令参数一致。
  if (baseline[handler]) {
    failures.push(`${handler} 已迁移为 DTO，但仍在 baseline 里（请删除过期条目）`);
  }
  const tauri = commandParams.get(command);
  if (!tauri) continue;
  checkedCommands.add(command);
  const fields = dtoFields(dtoSource, body.type);
  if (fields === null) {
    failures.push(`${handler} 的 DTO ${body.type} 在 dto.rs 里找不到`);
    continue;
  }
  const onlyTauri = tauri.filter((name) => !fields.includes(name));
  const onlyDto = fields.filter((name) => !tauri.includes(name));
  if (onlyTauri.length > 0 || onlyDto.length > 0) {
    failures.push(
      `${command}: Tauri 参数与 ${body.type} 不一致` +
        (onlyTauri.length ? `；仅 Tauri 有 [${onlyTauri.join(", ")}]` : "") +
        (onlyDto.length ? `；仅 DTO 有 [${onlyDto.join(", ")}]` : ""),
    );
  }
}

for (const [handler, reason] of Object.entries(baseline)) {
  if (!handlerNames.has(handler)) {
    failures.push(`${baselineRel} 登记了不存在的 handler: ${handler}`);
    continue;
  }
  if (!rawHandlers.has(handler)) {
    failures.push(`${baselineRel} 的 ${handler} 已不是 Json<Value>（请删除过期条目）`);
  }
  if (typeof reason !== "string" || reason.trim().length < 6) {
    failures.push(`${baselineRel} 的 ${handler} 缺少可读理由`);
  }
}

info.push(`POST ROUTES: ${[...routes.values()].filter((r) => r.method === "POST").length}`);
info.push(`已比对 DTO 的命令: ${checkedCommands.size}`);
info.push(`仍为 Json<Value> 的 handler: ${rawHandlers.size}（均已登记）`);

if (failures.length > 0) {
  console.error(`check-api-contract: FAIL（${failures.length} 处）`);
  for (const failure of failures) console.error("  - " + failure);
  for (const item of info) console.error("  · " + item);
  process.exit(1);
}
console.log("check-api-contract: PASS");
for (const item of info) console.log("  · " + item);

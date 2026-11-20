#!/usr/bin/env node
// WorkBuddy Shift: 三方契约一致性检查（无第三方依赖）。
//
// 检查 Tauri command 注册、前端 ROUTES 映射、axum 路由三者是否一一对应：
//   1. lib.rs generate_handler! 里的 commands::<name> 必须都有 #[tauri::command] 定义；
//   2. ROUTES 的 key（去掉 HTTP-only 的 switch_progress）必须都是已注册 command；
//   3. 除 desktop-only 命令外，每个 command 都必须在 ROUTES 里有 WebUI 映射；
//   4. ROUTES 的 {method,path} 必须都在 server router 里注册，且 router 没有多余路由；
//   5. router 引用的 handler 函数必须在 server api 源码里真实存在。
//
// 用法：node scripts/check-command-routes.mjs
// 退出码：0 通过；1 有缺失/多余/不一致。

import fs from "node:fs";
import path from "node:path";

const root = path.resolve(import.meta.dirname, "..");
const read = (rel) => fs.readFileSync(path.join(root, rel), "utf8");
const fail = [];
const info = [];

function filesUnder(rel) {
  const abs = path.join(root, rel);
  if (!fs.existsSync(abs)) return [];
  const stat = fs.statSync(abs);
  if (stat.isFile()) return [rel];
  const out = [];
  for (const entry of fs.readdirSync(abs, { withFileTypes: true })) {
    if (entry.isDirectory()) out.push(...filesUnder(path.join(rel, entry.name)));
    else if (entry.name.endsWith(".rs")) out.push(path.join(rel, entry.name));
  }
  return out;
}

function sourceText(directoryOrFile) {
  return filesUnder(directoryOrFile).map(read).join("\n");
}

// 1) lib.rs 里注册的 command 名称（含 companion:: 插件命令）。
const libRs = read("src-tauri/src/lib.rs");
const handlerBlock = libRs.match(/generate_handler!\[([\s\S]*?)\]/);
if (!handlerBlock) {
  fail.push("lib.rs 里找不到 generate_handler![...]");
}
const registered = handlerBlock
  ? [...handlerBlock[1].matchAll(/([a-z_]+)::([a-z0-9_]+)/g)].map((m) => m[2])
  : [];
const registeredSet = new Set(registered);
info.push("registered commands: " + registeredSet.size);

// 2) 实际 #[tauri::command] 定义。
const commandSources =
  filesUnder("src-tauri/src/commands").length > 0
    ? sourceText("src-tauri/src/commands")
    : existingSource("src-tauri/src/commands.rs");
function existingSource(rel) {
  return fs.existsSync(path.join(root, rel)) ? read(rel) : "";
}
const definitions = new Set(
  [...commandSources.matchAll(/#\[tauri::command(?:\([^\]]*\))?\]\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+([a-zA-Z0-9_]+)/g)].map(
    (m) => m[1],
  ),
);
info.push("command definitions: " + definitions.size);
for (const name of registeredSet) {
  if (!definitions.has(name) && !name.startsWith("get_companion") && !name.startsWith("set_companion") && !name.startsWith("open_companion")) {
    fail.push("generate_handler 注册了未定义的 command: " + name);
  }
}
for (const name of definitions) {
  if (!registeredSet.has(name)) fail.push("command 定义未注册进 generate_handler: " + name);
}

// 3) 前端 ROUTES 表。
const coreTs = read("src/lib/api/core.ts");
const routesBlock = coreTs.match(/const ROUTES: Record<string, Route> = \{([\s\S]*?)\n\};/);
if (!routesBlock) fail.push("core.ts 里找不到 ROUTES 表");
const routes = routesBlock
  ? [...routesBlock[1].matchAll(/([a-zA-Z0-9_]+)\s*:\s*\{\s*method:\s*"(GET|POST)"\s*,\s*path:\s*"([^"]+)"\s*,?\s*\}/g)].map((m) => ({
      command: m[1],
      method: m[2],
      path: m[3],
    }))
  : [];
info.push("ROUTES entries: " + routes.length);
for (const route of routes) {
  if (!registeredSet.has(route.command) && route.command !== "switch_progress") {
    fail.push("ROUTES 映射了不存在的 command: " + route.command);
  }
}

// HTTP 专属命令（没有 Tauri command，只由 webui 轮询）。
const HTTP_ONLY = new Set(["switch_progress"]);
// 桌面专属命令：没有 WebUI 路由，前端用 isDesktop()/isWebui() 守卫。
const DESKTOP_ONLY = new Set([
  "automation_sync_enabled",
  "automation_sync_set_enabled",
  "automation_tasks_summary",
  "dev_environment",
  "open_permission_settings",
  "check_auth_permission",
  "reveal_app_in_finder",
  "update_state",
  "update_download",
  "update_restart",
  "relaunch_app",
  "get_launch_at_login_enabled",
  "set_launch_at_login_enabled",
  "log_error",
  "get_error_log_path",
  "reveal_error_log",
  "get_companion_enabled",
  "set_companion_enabled",
  "open_companion_settings",
  // xlsx 二进制保存：桌面弹保存框后写入；WebUI 用浏览器 Blob 下载，不映射路由。
  "save_bytes_file",
]);
const mapped = new Set(routes.map((r) => r.command));
for (const name of registeredSet) {
  if (!mapped.has(name) && !DESKTOP_ONLY.has(name)) {
    fail.push("command 既没有 ROUTES 映射也不在 desktop-only 清单: " + name);
  }
}
for (const name of mapped) {
  if (DESKTOP_ONLY.has(name)) fail.push("desktop-only command 不应出现在 ROUTES: " + name);
}
for (const name of HTTP_ONLY) {
  if (registeredSet.has(name)) fail.push("HTTP-only 命令不应注册为 Tauri command: " + name);
  if (!mapped.has(name)) fail.push("HTTP-only 命令缺少 ROUTES 映射: " + name);
}

// 4) server router：解析 .route("...", get(x).post(y)) 形式。
const apiSources =
  filesUnder("crates/wb-shift-server/src/api").length > 0
    ? sourceText("crates/wb-shift-server/src/api")
    : existingSource("crates/wb-shift-server/src/api.rs");
const compact = apiSources.replace(/\s+/g, " ");
const serverRoutes = [];
let index = 0;
while ((index = compact.indexOf(".route(", index)) !== -1) {
  const start = index + ".route(".length;
  let depth = 1;
  let cursor = start;
  while (cursor < compact.length && depth > 0) {
    const ch = compact[cursor];
    if (ch === "(") depth += 1;
    else if (ch === ")") depth -= 1;
    cursor += 1;
  }
  const expression = compact.slice(start, cursor - 1);
  const pathMatch = expression.match(/^\s*"([^"]+)"/);
  if (!pathMatch) {
    fail.push("无法解析 .route 路径: " + expression.slice(0, 80));
    index = cursor;
    continue;
  }
  const routePath = pathMatch[1];
  const methods = [...expression.matchAll(/\b(get|post)\(\s*([a-zA-Z0-9_]+)/g)];
  if (methods.length === 0) fail.push("路由没有 get/post handler: " + routePath);
  for (const method of methods) {
    serverRoutes.push({ method: method[1].toUpperCase(), path: routePath, handler: method[2] });
  }
  index = cursor;
}
info.push("server routes: " + serverRoutes.length);

const serverKey = (method, routePath) => method + " " + routePath;
const serverKeys = new Set(serverRoutes.map((r) => serverKey(r.method, r.path)));
const routeKeys = new Set(routes.map((r) => serverKey(r.method, r.path)));
for (const route of routes) {
  if (!serverKeys.has(serverKey(route.method, route.path))) {
    fail.push("ROUTES 路径在 server router 缺失: " + route.method + " " + route.path + " (command " + route.command + ")");
  }
}
for (const route of serverRoutes) {
  if (!routeKeys.has(serverKey(route.method, route.path))) {
    fail.push("server router 路径没有 ROUTES 映射: " + serverKey(route.method, route.path) + " -> " + route.handler);
  }
}
if (serverRoutes.length !== routeKeys.size) {
  info.push("server routes=" + serverRoutes.length + " unique routes=" + routeKeys.size);
}

// 5) router 引用的 handler 函数必须在 api 源码里定义。
const handlerDefs = new Set(
  [...apiSources.matchAll(/(?:pub\(crate\)\s+)?(?:pub\s+)?(?:async\s+)?fn\s+([a-zA-Z0-9_]+)/g)].map((m) => m[1]),
);
for (const route of serverRoutes) {
  if (!handlerDefs.has(route.handler)) {
    fail.push("router 引用了未定义的 handler: " + route.handler + " (" + serverKey(route.method, route.path) + ")");
  }
}

if (fail.length > 0) {
  console.error("check-command-routes: FAIL");
  for (const item of fail) console.error("  - " + item);
  for (const item of info) console.error("  · " + item);
  process.exit(1);
}
console.log("check-command-routes: PASS (" + registeredSet.size + " commands, " + routes.length + " ROUTES, " + serverRoutes.length + " server routes)");
for (const item of info) console.log("  · " + item);

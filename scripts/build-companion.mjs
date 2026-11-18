import fs from 'node:fs/promises';
import path from 'node:path';
import { execFileSync } from 'node:child_process';

// WorkBuddy Shift 内置会话悬浮窗的构建脚本。
//
// 悬浮窗由两部分组成：
// - companion-ui/：Vite 前端工程，产出嵌入到宿主窗口的静态资源（两个页面）；
// - crates/companion-runtime/：本地监控运行时，作为 Tauri externalBin 随应用打包。
//
// 本脚本取代了旧的脚本各自处理 vendor 快照的做法：产物统一落在仓库 public/ 与
// src-tauri/binaries/（两者都已 gitignore），companion-ui/ 里不会再出现构建产物。
//
// Modes:
//   dev        — 产物比输入旧时才重建嵌入包与运行时（开发启动用，秒过）
//   build      — 强制重建嵌入包与运行时（发布构建用）
//   demo-dev   — 产物过期时重建公开演示包到 public/companion-demo/
//   demo-build — 先 clean 再重建公开演示包
//   clean      — 删除 public/companion-demo/（生产构建前清掉演示产物）
//
// --force 跳过所有时间戳判断。

const MODES = ['dev', 'build', 'demo-dev', 'demo-build', 'clean'];
const argv = process.argv.slice(2);
const force = argv.includes('--force');
const positional = argv.filter((arg) => arg !== '--force');
const mode = positional[0] ?? 'dev';
if (positional.length > 1 || !MODES.includes(mode)) {
  throw new Error(`Usage: node scripts/build-companion.mjs <${MODES.join('|')}> [--force]`);
}

const root = path.resolve(import.meta.dirname, '..');
const ui = path.join(root, 'companion-ui');
const embedOutput = path.join(root, 'public', 'companion');
const demoOutput = path.join(root, 'public', 'companion-demo');
const binaries = path.join(root, 'src-tauri', 'binaries');
const iconSource = path.join(root, 'public', 'icon.png');

const isWindows = process.platform === 'win32';
/** dev 之外，build 模式也必须无条件重建，不依赖 mtime。 */
const forceBuild = force || mode === 'build';

const WEB_ENTRIES = [
  'src',
  'public',
  'desktop.html',
  'desktop-settings.html',
  'demo.html',
  'index.html',
  'vite.config.js',
  'package.json',
  'tsconfig.json',
].map((entry) => path.join(ui, entry));

function log(message) {
  console.log(`[companion] ${message}`);
}

async function exists(target) {
  try {
    await fs.access(target);
    return true;
  } catch {
    return false;
  }
}

/**
 * 目录树里最新的修改时间（毫秒），不存在返回 0。
 *
 * 跳过 node_modules / dist 等产物目录：它们体量大且构建后永远比源码新，
 * 纳入比较会让缓存永远失效。
 */
async function newestMtime(target, skip = new Set(['node_modules', 'dist', 'dist-embed', 'dist-demo', '.git', 'target'])) {
  let stat;
  try {
    stat = await fs.stat(target);
  } catch {
    return 0;
  }
  if (stat.isFile()) return stat.mtimeMs;
  let newest = stat.mtimeMs;
  for (const entry of await fs.readdir(target, { withFileTypes: true })) {
    if (skip.has(entry.name)) continue;
    newest = Math.max(newest, await newestMtime(path.join(target, entry.name), skip));
  }
  return newest;
}

async function newestOf(targets) {
  let newest = 0;
  for (const target of targets) newest = Math.max(newest, await newestMtime(target));
  return newest;
}

// Windows 的 pnpm 是 .cmd，Node 20+ 禁止 execFile 直接启动 .cmd，必须经 shell 解析。
function runPnpm(args) {
  execFileSync('pnpm', args, { cwd: root, stdio: 'inherit', shell: isWindows });
}

/**
 * 定位 Rust 工具链里的可执行文件。
 *
 * 优先 PATH；找不到时按开发者的常见安装位置兜底（与 scripts/dev-start.ps1 一致），
 * 这样从任意终端直接 `node scripts/build-companion.mjs dev` 也能跑通。
 */
function resolveTool(name) {
  const executable = isWindows ? `${name}.exe` : name;
  const candidates = [];
  if (process.env[name.toUpperCase()]) candidates.push(process.env[name.toUpperCase()]);
  for (const dir of (process.env.PATH ?? '').split(path.delimiter)) {
    if (dir) candidates.push(path.join(dir, executable));
  }
  for (const dir of [
    process.env.USERPROFILE && path.join(process.env.USERPROFILE, '.cargo', 'bin'),
    process.env.HOME && path.join(process.env.HOME, '.cargo', 'bin'),
    isWindows && 'D:\\Rust\\cargo\\bin',
    isWindows && 'C:\\Rust\\cargo\\bin',
  ]) {
    if (dir) candidates.push(path.join(dir, executable));
  }
  for (const candidate of candidates) {
    try {
      execFileSync(candidate, ['--version'], { stdio: 'ignore' });
      return candidate;
    } catch {
      // 继续找下一个
    }
  }
  return executable;
}

/** target 取 AGENT_COMPANION_TARGET，缺省用 rustc 的 host；发布工作流按平台注入。 */
function resolveTarget() {
  const explicit = process.env.AGENT_COMPANION_TARGET?.trim();
  if (explicit) return explicit;
  const rustc = resolveTool('rustc');
  const host = execFileSync(rustc, ['-vV'], { encoding: 'utf8' }).match(/^host: (.+)$/m)?.[1];
  if (!host) throw new Error('无法确定 Rust target，请设置 AGENT_COMPANION_TARGET');
  return host;
}

/**
 * companion-ui 的依赖由仓库根的 pnpm workspace 统一安装，这里只做存在性检查。
 *
 * 不再自己比对锁文件：pnpm 12 会把固定版本的 pnpm 写进 `packageManagerDependencies`，
 * 而安装态副本（node_modules/.pnpm/lock.yaml）不会因包管理器升级而重写，按内容比较
 * 必然误报。依赖是否与锁文件同步交给 pnpm：`pnpm run` / `pnpm exec` 的
 * verifyDepsBeforeRun（默认 install）会先检查并在过期时安装。
 *
 * 这里刻意不代跑安装：`pnpm run *` 自身就在 pnpm 进程里，嵌套 `pnpm install`
 * 会改写正在使用的 node_modules（Windows 上还可能与 store 锁冲突）。缺依赖时
 * 直接给出一条可执行的命令，比静默重装更容易排查。
 */
async function requireDeps() {
  if (!(await exists(path.join(ui, 'node_modules')))) {
    throw new Error('companion-ui 依赖未安装：先在仓库根目录运行 pnpm install');
  }
}

/** 把宿主应用图标复制成嵌入页面的 favicon（HTML 里指向 ./icon.png）。 */
async function copyIcon(destination) {
  await fs.mkdir(destination, { recursive: true });
  await fs.copyFile(iconSource, path.join(destination, 'icon.png'));
}

async function requireEntries(directory, entries) {
  for (const entry of entries) await fs.access(path.join(directory, entry));
}

/** 嵌入包是否过期：以 public/companion/desktop-settings.html 作为 marker。 */
async function webStale() {
  if (forceBuild) return true;
  const marker = path.join(embedOutput, 'desktop-settings.html');
  if (!(await exists(marker))) return true;
  const markerTime = (await fs.stat(marker)).mtimeMs;
  return (await newestOf(WEB_ENTRIES)) > markerTime;
}

async function buildEmbed() {
  await requireDeps();
  // outDir 在 companion-ui 的工程根之外（public/ 已 gitignore）。Vite 不会自动清空
  // 工程根外的 outDir，这里先删干净，避免旧 chunk 残留。
  await fs.rm(embedOutput, { recursive: true, force: true });
  log('构建嵌入包 → public/companion');
  // 用相对 companion-ui 根目录的 outDir 锁定输出位置，避免落进 companion-ui/。
  runPnpm(['-C', 'companion-ui', 'run', 'build:embed', '--base', './', '--emptyOutDir', '--outDir', '../public/companion']);
  await requireEntries(embedOutput, ['desktop.html', 'desktop-settings.html']);
  await copyIcon(embedOutput);
}

let RUNTIME_TARGET = '';
let RUNTIME_SUFFIX = '';
let RUNTIME_BINARY = '';
let RUNTIME_BUILT = '';

function resolveRuntimePaths() {
  RUNTIME_TARGET = resolveTarget();
  RUNTIME_SUFFIX = RUNTIME_TARGET.includes('windows') ? '.exe' : '';
  RUNTIME_BINARY = path.join(binaries, `companion-runtime-${RUNTIME_TARGET}${RUNTIME_SUFFIX}`);
  RUNTIME_BUILT = path.join(root, 'target', RUNTIME_TARGET, 'release', `companion-runtime${RUNTIME_SUFFIX}`);
}

/** 运行时二进制是否过期：比 crates/companion-* 与依赖清单旧。 */
async function runtimeStale() {
  if (forceBuild) return true;
  if (!(await exists(RUNTIME_BINARY))) return true;
  const markers = [(await fs.stat(RUNTIME_BINARY)).mtimeMs];
  // cargo 对未变化的构建可能不重写产物；取副本与 cargo 产物里较新的那个。
  if (await exists(RUNTIME_BUILT)) markers.push((await fs.stat(RUNTIME_BUILT)).mtimeMs);
  const inputs = await newestOf([
    path.join(root, 'Cargo.toml'),
    path.join(root, 'Cargo.lock'),
    path.join(root, 'crates', 'companion-core'),
    path.join(root, 'crates', 'companion-runtime'),
    path.join(root, 'crates', 'companion-desktop'),
  ]);
  return inputs > Math.max(...markers);
}

async function buildRuntime() {
  log(`编译运行时 companion-runtime（release, ${RUNTIME_TARGET}）`);
  execFileSync(
    resolveTool('cargo'),
    ['build', '--release', '--locked', '-p', 'companion-runtime', '--target', RUNTIME_TARGET],
    { cwd: root, stdio: 'inherit' },
  );
  await fs.mkdir(binaries, { recursive: true });
  await fs.copyFile(RUNTIME_BUILT, RUNTIME_BINARY);
  log(`运行时二进制 → ${path.relative(root, RUNTIME_BINARY)}`);
}

/** 演示包是否过期：以 public/companion-demo/desktop.html 作为 marker。 */
async function demoStale() {
  const marker = path.join(demoOutput, 'desktop.html');
  if (!(await exists(marker))) return true;
  const markerTime = (await fs.stat(marker)).mtimeMs;
  return (await newestOf(WEB_ENTRIES)) > markerTime;
}

async function buildDemo(always) {
  if (!always && !force && !(await demoStale())) {
    log('演示包比源码新，跳过构建（--force 可强制重建）');
    await copyIcon(demoOutput);
    return;
  }
  await requireDeps();
  await fs.rm(demoOutput, { recursive: true, force: true });
  log('构建演示包 → public/companion-demo');
  runPnpm(['-C', 'companion-ui', 'run', 'build:demo', '--base', './', '--emptyOutDir', '--outDir', '../public/companion-demo']);
  // 演示页需要两份产物都在：舞台页（demo.html）与悬浮栏页（desktop.html）。
  await requireEntries(demoOutput, ['index.html', 'demo.html', 'desktop.html']);
  await copyIcon(demoOutput);
}

if (mode === 'clean') {
  await fs.rm(demoOutput, { recursive: true, force: true });
  log('已删除 public/companion-demo/');
} else if (mode === 'demo-dev' || mode === 'demo-build') {
  await buildDemo(mode === 'demo-build');
} else {
  if (mode === 'build') {
    // 生产构建前清掉演示产物：它们只服务 GitHub Pages，不应随安装包或 webui 分发。
    await fs.rm(demoOutput, { recursive: true, force: true });
  }
  resolveRuntimePaths();
  if (await webStale()) {
    await buildEmbed();
  } else {
    log('嵌入包比源码新，跳过构建（--force 可强制重建）');
    await copyIcon(embedOutput);
  }
  if (await runtimeStale()) {
    await buildRuntime();
  } else {
    log(`运行时二进制比源码新，跳过 release 构建（${path.relative(root, RUNTIME_BINARY)}）`);
  }
}

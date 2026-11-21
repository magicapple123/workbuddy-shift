// workbuddy-shift postinstall：从「平台包」复制本平台二进制。
//
// 平台分包（esbuild 模式）：二进制发布在独立 npm 包（workbuddy-shift-<platform>-<arch>），
// 主包声明为 optionalDependencies，安装时 npm 自动装好平台包，postinstall 只需复制——
// 不依赖 GitHub，国内镜像（npmmirror）也能稳定安装。
//
// 两个渠道，按顺序尝试，第一个「能 resolve 且 bin/<file> 存在」的胜出：
//   1. workbuddy-shift-<platform>-<arch>                  npmjs（主渠道，先试原名）
//   2. @magicapple123/workbuddy-shift-<platform>-<arch>    GitHub Packages（scoped 镜像）
// 镜像只是备用渠道：GitHub Packages 安装即使公开包也要 token，主渠道仍是 npmjs。
//
// 环境变量覆盖：
//   WB_SHIFT_BINARY=<本地二进制路径>  本地开发/离线安装（直接复制，不联网）
const fs = require("fs");
const path = require("path");

// 为什么把 scope 硬编码：镜像固定由官方账号 @magicapple123 发布（GitHub Packages 只接受
// scoped 包名，scope 只能是仓库 owner）。fork 后要改这一处，并同步
// scripts/publish-github-packages.sh 的 <scope> 参数。
const MIRROR_SCOPE = "@magicapple123";

const FILE = {
  "darwin-arm64": "workbuddy-shift-darwin-arm64",
  "darwin-x64": "workbuddy-shift-darwin-x64",
  "win32-x64": "workbuddy-shift-win32-x64.exe",
  "linux-x64": "workbuddy-shift-linux-x64",
  "linux-arm64": "workbuddy-shift-linux-arm64",
}[`${process.platform}-${process.arch}`];

const PLATFORM_PKG = `workbuddy-shift-${process.platform}-${process.arch}`;

// 顺序即优先级：npmjs 主渠道在前，GitHub Packages 镜像兜底在后。
const PLATFORM_PKGS = [PLATFORM_PKG, `${MIRROR_SCOPE}/${PLATFORM_PKG}`];

if (!FILE) {
  console.warn(
    `workbuddy-shift: 跳过平台 ${process.platform}-${process.arch}（当前不支持），` +
      `可手动下载二进制后放置到 bin/ 目录`,
  );
  process.exit(0);
}

const binDir = path.join(__dirname, "..", "bin");
const target = path.join(binDir, FILE);

function fail(msg) {
  console.error(`workbuddy-shift install: ${msg}`);
  console.error(
    `请确认已安装对应平台包（${PLATFORM_PKGS.join(" 或 ")}，npm 安装主包时会自动装上），` +
      `或用 WB_SHIFT_BINARY 指向本地二进制。`,
  );
  process.exit(1);
}

function copyFrom(src) {
  fs.mkdirSync(binDir, { recursive: true });
  fs.copyFileSync(src, target);
  if (process.platform !== "win32") fs.chmodSync(target, 0o755);
  const size = fs.statSync(target).size;
  if (size < 1 * 1024 * 1024) {
    fs.unlinkSync(target);
    return fail(`平台包二进制异常（仅 ${size} 字节）`);
  }
  console.log(
    `workbuddy-shift: 二进制就绪 → ${target} (${(size / 1048576).toFixed(1)}MB)`,
  );
}

/**
 * 按 PLATFORM_PKGS 顺序寻找平台二进制。
 * @returns {{src: string, pkg: string, notes: string[]}} src 为空表示全部渠道都失败，
 *   notes 记录每个渠道失败的原因（用于报错文案）。
 */
function findPlatformBinary() {
  const notes = [];
  for (const pkg of PLATFORM_PKGS) {
    let pkgRoot;
    try {
      // 只解析 package.json：包存在即可定位根目录，不需要入口文件可执行。
      pkgRoot = path.dirname(require.resolve(`${pkg}/package.json`));
    } catch (e) {
      notes.push(`${pkg} 未安装`);
      continue;
    }
    const src = path.join(pkgRoot, "bin", FILE);
    if (fs.existsSync(src)) return { src, pkg, notes };
    notes.push(`${pkg} 中未找到 bin/${FILE}`);
  }
  return { src: "", pkg: "", notes };
}

async function main() {
  // 1) 本地二进制覆盖（开发/离线）
  if (process.env.WB_SHIFT_BINARY) {
    const local = path.resolve(process.env.WB_SHIFT_BINARY);
    if (fs.existsSync(local)) return copyFrom(local);
    return fail(`WB_SHIFT_BINARY 指向的文件不存在: ${local}`);
  }

  // 2) 从平台包复制（npmjs 主渠道 → GitHub Packages 镜像）
  const found = findPlatformBinary();
  if (found.src) return copyFrom(found.src);

  // 3) 渠道都不可用：可能是 optionalDependencies 没装上（如手动安装/旧版 npm），
  //    但二进制此前已经复制过——此时不当作失败。
  if (fs.existsSync(target)) {
    console.log("workbuddy-shift: 二进制已存在，跳过");
    return;
  }
  return fail(`平台包未安装（${found.notes.join("；")}）。请重新执行 npm install。`);
}

main();

#!/bin/sh
# 把 npmjs 上已发布的 6 个包（主包 + 5 个平台包）镜像成 scoped 包，发布到 GitHub Packages。
#
# 为什么需要这套镜像：GitHub Packages 的 npm registry 只接受 scoped 包名（@owner/name），
# 所以镜像包名一律是 @<scope>/<原包名>。它只是**备用渠道**——GitHub Packages 即使对公开包
# 也要求 token 才能安装，主渠道永远是 npmjs 上的 workbuddy-shift。
#
# 用法：
#   sh scripts/publish-github-packages.sh <version> <scope> <owner/repo> [--dry-run]
# 示例：
#   NODE_AUTH_TOKEN=ghp_xxx sh scripts/publish-github-packages.sh \
#     0.2.5 magicapple123 magicapple123/workbuddy-shift
#
# 做法（全部在 mktemp 出来的临时目录里完成，结束即清理，绝不改仓库文件）：
#   1. 从 npmjs 拉取 6 个包的 tarball（npm pack <name>@<version>）；
#   2. 解包改写 package.json：name 加 scope、补 repository、补 publishConfig.registry；
#      主包的 optionalDependencies 五个键一并改成 scoped 名（版本保持 <version>）；
#   3. 主包额外用**仓库当前版本**的 npm/scripts/install.js 覆盖 tarball 里的那份：
#      已发到 npmjs 的旧版本（0.2.5）里的 install.js 还不认 scoped 平台包，覆盖后镜像自洽；
#      未来版本本身就是新版 install.js，覆盖等于无操作；
#   4. 重新打包（保留 bin/<file> 的可执行位与 files 白名单）；
#   5. 探测 GitHub Packages 上是否已存在同版本：存在则 skip（幂等，重跑不炸）；
#   6. npm publish 到 https://npm.pkg.github.com。
#
# 环境变量：
#   NODE_AUTH_TOKEN  发布用的 GitHub token（classic PAT 需 write:packages；CI 里用
#                    GITHUB_TOKEN 的 packages: write）。--dry-run 不需要。
set -eu

GH_REGISTRY="https://npm.pkg.github.com"
NPM_REGISTRY="https://registry.npmjs.org"

# 平台清单：与 npm/package.json 的 optionalDependencies、npm/platform/ 目录、
# AGENTS.md §3「npm 平台包映射」保持一致。
PLATFORMS="darwin-arm64 darwin-x64 win32-x64 linux-x64 linux-arm64"

usage() {
  cat <<'USAGE'
用法: sh scripts/publish-github-packages.sh <version> <scope> <owner/repo> [--dry-run]

  <version>     npmjs 上已发布的版本号（不带 v），例如 0.2.5
  <scope>       GitHub Packages 的 scope（= 仓库 owner），例如 magicapple123
  <owner/repo>  包要关联的仓库，例如 magicapple123/workbuddy-shift
  --dry-run     只做下载/改写/重打包，打印将要发布的包与差异摘要；不探测、不发布

环境变量:
  NODE_AUTH_TOKEN  发布用 GitHub token（packages: write）；--dry-run 不需要
USAGE
}

err() {
  echo "publish-github-packages: $*" >&2
}

# 平台 tag → 平台包里的二进制文件名（与 install.js 的 FILE 映射一致，两处都要改）。
bin_for() {
  case "$1" in
    win32-x64) echo "workbuddy-shift-win32-x64.exe" ;;
    *) echo "workbuddy-shift-$1" ;;
  esac
}

# ---------------------------------------------------------------------------
# 1. 参数
# ---------------------------------------------------------------------------
VERSION=""
SCOPE=""
REPO=""
DRY_RUN=0
POS=0

for arg in "$@"; do
  case "$arg" in
    --dry-run) DRY_RUN=1 ;;
    -h | --help)
      usage
      exit 0
      ;;
    -*)
      err "未知选项: $arg"
      usage >&2
      exit 2
      ;;
    *)
      POS=$((POS + 1))
      case "$POS" in
        1) VERSION="$arg" ;;
        2) SCOPE="$arg" ;;
        3) REPO="$arg" ;;
        *)
          err "多余参数: $arg"
          usage >&2
          exit 2
          ;;
      esac
      ;;
  esac
done

if [ -z "$VERSION" ] || [ -z "$SCOPE" ] || [ -z "$REPO" ]; then
  err "参数不足"
  usage >&2
  exit 2
fi

# 归一化：容忍 v 前缀、@scope 前缀、完整仓库 URL 与 .git 后缀。
# scoped 包名必须全小写（npm 规范），GitHub 的 owner/repo 大小写不敏感，故一并小写。
RAW_VERSION="$VERSION" # 报错文案用原始输入：剥掉 v 之后再看会变成 "er" 这种怪串
VERSION=${VERSION#v}
SCOPE=$(printf '%s' "${SCOPE#@}" | tr 'A-Z' 'a-z')
REPO=$(printf '%s' "$REPO" | tr 'A-Z' 'a-z')
REPO=${REPO#https://github.com/}
REPO=${REPO#http://github.com/}
REPO=${REPO#git@github.com:}
REPO=${REPO%/}
REPO=${REPO%.git}

# 形状：三段数字（允许 -rc.N / +build 这类 semver 后缀）。版本会直接拼进
# npm pack 的 spec，形状不对就在发布前失败，别等到 npm 报晦涩错误。
case "$VERSION" in
  [0-9]*.[0-9]*.[0-9]*) : ;;
  *)
    err "版本号看起来不对: $RAW_VERSION（期望 x.y.z 或 x.y.z-rc.N）"
    exit 2
    ;;
esac
# 字符集：只允许 semver 的 [0-9A-Za-z.-+]，挡住 ; & 空格 这类会被 npm 当成
# 版本号分隔符/命令注入物的输入（例如 "0.2.5;x"）。
case "$VERSION" in
  *[!0-9A-Za-z.+-]*)
    err "版本号只能包含字母/数字/./-/+: $RAW_VERSION"
    exit 2
    ;;
esac
case "$SCOPE" in
  *[!a-z0-9._-]*)
    err "scope 只允许小写字母/数字/./_/-: $SCOPE"
    exit 2
    ;;
esac
case "$REPO" in
  */*) : ;;
  *)
    err "<owner/repo> 格式错误: $REPO"
    exit 2
    ;;
esac

if [ "$DRY_RUN" -eq 0 ] && [ -z "${NODE_AUTH_TOKEN:-}" ]; then
  err "缺少 NODE_AUTH_TOKEN：GitHub Packages 发布必须带 token（classic PAT 需 write:packages，"
  err "CI 用 GITHUB_TOKEN 的 packages: write）。例如："
  err "  NODE_AUTH_TOKEN=ghp_xxx sh scripts/publish-github-packages.sh $VERSION $SCOPE $REPO"
  exit 1
fi

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
INSTALL_JS="$REPO_ROOT/npm/scripts/install.js"
if [ ! -f "$INSTALL_JS" ]; then
  err "找不到 $INSTALL_JS（脚本需要在仓库内运行）"
  exit 1
fi
command -v node >/dev/null 2>&1 || {
  err "需要 node（用于改写 package.json）"
  exit 1
}

PLAT_PKGS=""
for plat in $PLATFORMS; do
  PLAT_PKGS="$PLAT_PKGS workbuddy-shift-$plat"
done
# 平台包先发、主包最后发：主包的 optionalDependencies 指向 scoped 平台包，
# 反过来的话发布过程中会出现短暂「主包引用了还不存在的依赖」。
ALL_PKGS="$PLAT_PKGS workbuddy-shift"

# ---------------------------------------------------------------------------
# 2. 临时工作区（结束即清理，绝不写仓库）
# ---------------------------------------------------------------------------
TMP=$(mktemp -d "${TMPDIR:-/tmp}/wb-shift-ghpkg.XXXXXX")
cleanup() {
  rm -rf "$TMP"
}
trap cleanup EXIT
trap 'cleanup; exit 130' INT
trap 'cleanup; exit 143' TERM

mkdir -p "$TMP/tarballs" "$TMP/pkg" "$TMP/dist"

# 临时 .npmrc：只在临时目录里，不落仓库。${NODE_AUTH_TOKEN} 由 npm 在读取配置时展开。
NPMRC="$TMP/npmrc"
{
  echo "//npm.pkg.github.com/:_authToken=\${NODE_AUTH_TOKEN}"
  echo "@$SCOPE:registry=$GH_REGISTRY"
} >"$NPMRC"

# 改写 package.json 的 node 脚本（不依赖 jq）。
cat >"$TMP/rewrite.js" <<'NODEJS'
// 把 npmjs 的 package.json 改写成 GitHub Packages 镜像用的版本。
// 参数: <pkgDir> <scope> <owner/repo> <kind: main|platform> <version> [<binFile>]
// 只跑在 mktemp 的临时目录里；kind=platform 时不动 os / cpu / files / bin。
const fs = require("fs");
const path = require("path");

const argv = process.argv.slice(2);
const [pkgDir, scope, repo, kind, version, binFile] = argv;
if (!pkgDir || !scope || !repo || !kind || !version) {
  console.error("rewrite.js: 参数不足");
  process.exit(2);
}

const manifestPath = path.join(pkgDir, "package.json");
const pkg = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
const originalName = pkg.name;

// 1) scoped 包名（GitHub Packages 只接受 @owner/name，且必须小写）
pkg.name = `@${scope}/${originalName}`;

// 2) repository：GitHub Packages 靠它把包与仓库关联（决定包出现在哪个仓库的 Packages 页）
pkg.repository = { type: "git", url: `https://github.com/${repo}.git` };

// 3) publishConfig：包自己声明镜像 registry，避免误发到 npmjs 主渠道
pkg.publishConfig = Object.assign({}, pkg.publishConfig, {
  registry: "https://npm.pkg.github.com",
});

// 4) 主包的 optionalDependencies 改成 scoped 名（版本保持 <version>），
//    这样 scoped 主包只会去 GitHub Packages 装 scoped 平台包，不会混入 npmjs 的同名包。
const scopedDeps = [];
if (kind === "main") {
  const deps = pkg.optionalDependencies || {};
  const next = {};
  for (const dep of Object.keys(deps)) {
    const scoped = `@${scope}/${dep}`;
    next[scoped] = version;
    scopedDeps.push(`${scoped}@${version}`);
  }
  pkg.optionalDependencies = next;
}

fs.writeFileSync(manifestPath, `${JSON.stringify(pkg, null, 2)}\n`);

// 摘要输出：dry-run 直接展示给用户看，正式发布时调用方丢弃。
console.log(`  name          : ${originalName} → ${pkg.name}`);
console.log(`  version       : ${pkg.version}`);
console.log(`  repository    : ${JSON.stringify(pkg.repository)}`);
console.log(`  publishConfig : ${JSON.stringify(pkg.publishConfig)}`);
if (kind === "main") {
  console.log(`  optionalDeps  : ${scopedDeps.join(", ") || "(空)"}`);
} else {
  console.log(
    `  os/cpu        : ${JSON.stringify(pkg.os)} / ${JSON.stringify(pkg.cpu)}（不改）`,
  );
  console.log(`  files         : ${JSON.stringify(pkg.files)}（不改）`);
  console.log(`  bin           : bin/${binFile}（不改）`);
}
NODEJS

# npm pack 的包装：输出落到 --pack-destination 的 tarball 绝对路径。
# 用 --json 取文件名，避免把 npm notice 输出误当成文件名。
#
# 重试：本脚本紧跟在 publish-main 之后跑，主包刚发到 npmjs 时 registry 还没传播完，
# `npm pack workbuddy-shift@X` 会报 `ETARGET / notarget`（v0.2.6 首次踩到，整个
# mirror job 变红）。这属于**时间差**而不是包缺失，所以对 ETARGET 做有界重试；
# 其它错误（404 之外的鉴权/网络）仍立刻失败，不要用重试掩盖真实问题。
#
# 窗口给到 5 分钟（20×15s）：npm 在 publish 成功后会提示「may take a few minutes」，
# 0.3.0-rc.1 实测在网络正常的情况下仍用了 2 分钟以上才可解析，8×15s 不够。
# 只在备用渠道上多等几分钟，好过让整个 build run 变红。
pack() { # $1 = 包 spec 或本地目录, $2 = 目的目录
  attempt=1
  max_attempts=20
  while :; do
    if npm pack "$1" --pack-destination "$2" --registry="$NPM_REGISTRY" \
      --json >"$TMP/pack.json" 2>"$TMP/pack.log"; then
      break
    fi
    if [ "$attempt" -lt "$max_attempts" ] && grep -q "notarget" "$TMP/pack.log"; then
      printf '[publish-github-packages] %s 尚未在 npmjs 可见（ETARGET），%ss 后重试（%s/%s）\n' \
        "$1" 15 "$attempt" "$max_attempts" >&2
      attempt=$((attempt + 1))
      sleep 15
      continue
    fi
    err "npm pack 失败: $1"
    sed 's/^/    /' "$TMP/pack.log" >&2
    exit 1
  done
  filename=$(node -e '
    const fs = require("fs");
    const list = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
    if (!list.length || !list[0].filename) {
      console.error("npm pack --json 输出没有 filename");
      process.exit(1);
    }
    process.stdout.write(list[0].filename);
  ' "$TMP/pack.json")
  printf '%s/%s\n' "$2" "$filename"
}

# tarball 大小（KB），仅用于输出可读性。
tarball_kb() {
  bytes=$(wc -c <"$1")
  echo $((bytes / 1024))
}

# 校验重打包产物里关键文件仍在（files 白名单 / bin 不能被改写步骤带丢）。
check_members() { # $1 = tarball, $2 = kind, $3 = binFile
  members=$(tar -tzf "$1")
  for required in "package/package.json"; do
    case "$members" in
      *"$required"*) : ;;
      *)
        err "重打包产物缺少 $required: $1"
        exit 1
        ;;
    esac
  done
  if [ "$2" = "main" ]; then
    for required in "package/bin/workbuddy-shift.js" "package/scripts/install.js"; do
      case "$members" in
        *"$required"*) : ;;
        *)
          err "重打包产物缺少 $required: $1"
          exit 1
          ;;
      esac
    done
  else
    case "$members" in
      *"package/bin/$3"*) : ;;
      *)
        err "重打包产物缺少 package/bin/$3: $1"
        exit 1
        ;;
    esac
  fi
}

# 可执行位只是警告：Windows 开发机无法把 exec 位写进 tarball（脱包时也读不出来），
# Linux/CI 上会保留 755。功能上不依赖它——npm 安装 bin 时会 chmod，
# install.js 复制平台二进制后也会 chmod 0755。
check_exec_bit() { # $1 = tarball, $2 = 包内路径
  if tar -tvzf "$1" | grep -F " $2" | grep -q '^-rwx'; then
    return 0
  fi
  echo "  警告: $2 在 tarball 里没有可执行位（Windows 上打包的已知差异，Linux/CI 为 755）"
}

# ---------------------------------------------------------------------------
# 3. 阶段一：下载 → 改写 → 重打包（--dry-run 到此为止）
# ---------------------------------------------------------------------------
echo "镜像 $VERSION → $GH_REGISTRY（scope @$SCOPE，仓库 $REPO）"
if [ "$DRY_RUN" -eq 1 ]; then
  echo "模式: --dry-run（不探测、不发布）"
fi
echo ""

for name in $ALL_PKGS; do
  if [ "$name" = "workbuddy-shift" ]; then
    kind="main"
    binfile=""
  else
    kind="platform"
    binfile=$(bin_for "${name#workbuddy-shift-}")
  fi

  echo "[$name] 拉取 npmjs tarball"
  src_tgz=$(pack "$name@$VERSION" "$TMP/tarballs")

  pkg_dir="$TMP/pkg/$name"
  mkdir -p "$pkg_dir"
  # --strip-components=1：把 tarball 里的 package/* 平铺到 pkg_dir
  tar -xzf "$src_tgz" -C "$pkg_dir" --strip-components=1

  echo "[$name] 改写 package.json"
  node "$TMP/rewrite.js" "$pkg_dir" "$SCOPE" "$REPO" "$kind" "$VERSION" "$binfile" \
    >"$TMP/pkg/$name.summary"

  if [ "$kind" = "main" ]; then
    # 主包：镜像里的 install.js 必须认 scoped 平台包。已发布版本的 tarball 里是旧解析器，
    # 这里用仓库当前版本覆盖；未来版本本来就是新版，覆盖等于无操作。
    cp "$INSTALL_JS" "$pkg_dir/scripts/install.js"
    chmod 0755 "$pkg_dir/bin/workbuddy-shift.js" 2>/dev/null || true
    echo "  install.js    : 已用仓库当前版本覆盖（npm/scripts/install.js）" \
      >>"$TMP/pkg/$name.summary"
  else
    # 显式补回可执行位：从 tarball 脱包时可能已丢（Windows），重打包前补一次。
    chmod 0755 "$pkg_dir/bin/$binfile" 2>/dev/null || true
  fi

  echo "[$name] 重新打包"
  out_tgz=$(pack "$pkg_dir" "$TMP/dist")
  check_members "$out_tgz" "$kind" "$binfile"
  if [ "$kind" = "main" ]; then
    check_exec_bit "$out_tgz" "package/bin/workbuddy-shift.js" 2>/dev/null || true
  else
    check_exec_bit "$out_tgz" "package/bin/$binfile" 2>/dev/null || true
  fi
  printf '%s\n' "$out_tgz" >"$TMP/dist/$name.path"
  printf '%s\n' "$(basename "$out_tgz")" >"$TMP/dist/$name.filename"
  echo ""
done

if [ "$DRY_RUN" -eq 1 ]; then
  echo "以下 6 个包将被发布（--dry-run，未探测、未发布）："
  for name in $ALL_PKGS; do
    out_tgz=$(cat "$TMP/dist/$name.path")
    size_kb=$(tarball_kb "$out_tgz")
    echo ""
    echo "@$SCOPE/$name  ($(cat "$TMP/dist/$name.filename"), ${size_kb}KB)"
    cat "$TMP/pkg/$name.summary"
  done
  echo ""
  echo "发布后主包安装命令（备用渠道，需要 read:packages 的 GitHub token）："
  echo "  npm login --registry=$GH_REGISTRY"
  echo "  npm i -g @$SCOPE/workbuddy-shift --registry=$GH_REGISTRY"
  exit 0
fi

# ---------------------------------------------------------------------------
# 4. 阶段二：探测 → 发布（幂等）
# ---------------------------------------------------------------------------
echo "发布到 $GH_REGISTRY"
published=0
skipped=0

for name in $ALL_PKGS; do
  scoped="@$SCOPE/$name"
  out_tgz=$(cat "$TMP/dist/$name.path")
  size_kb=$(tarball_kb "$out_tgz")

  # 已存在则跳过：重跑同版本不该失败（CI 重跑、手动回填都依赖这一点）。
  if npm view "$scoped@$VERSION" version --registry="$GH_REGISTRY" \
    --userconfig="$NPMRC" >"$TMP/view.out" 2>&1; then
    printf 'skip    %-44s 已存在 %s（幂等跳过）\n' "$scoped" "$VERSION"
    skipped=$((skipped + 1))
    continue
  fi

  # 不加 npmjs 的 access 参数：GitHub Packages 的 registry 不认那套语义
  # （可见性由包设置 / 与仓库的关联决定），加上去可能直接报参数错误。
  if npm publish "$out_tgz" --registry="$GH_REGISTRY" \
    --userconfig="$NPMRC" >"$TMP/pub.out" 2>&1; then
    printf 'publish %-44s %s (%sKB)\n' "$scoped" "$VERSION" "$size_kb"
    published=$((published + 1))
    continue
  fi

  # 探测与发布之间的竞态：并发跑同一版本时会拿到 409，同样按 skip 处理。
  if grep -qE 'E409|EPUBLISHCONFLICT|409 Conflict' "$TMP/pub.out"; then
    printf 'skip    %-44s 已存在 %s（发布时 409，幂等跳过）\n' "$scoped" "$VERSION"
    skipped=$((skipped + 1))
    continue
  fi

  err "发布失败: $scoped@$VERSION"
  sed 's/^/    /' "$TMP/pub.out" >&2
  exit 1
done

echo ""
echo "完成：发布 $published 个，跳过 $skipped 个（已存在）。"
echo "主包安装命令（备用渠道，需要 read:packages 的 GitHub token）："
echo "  npm login --registry=$GH_REGISTRY"
echo "  npm i -g @$SCOPE/workbuddy-shift --registry=$GH_REGISTRY"
echo "（长期使用可把 @$SCOPE:registry 写进 ~/.npmrc，之后不必每次带 --registry）"

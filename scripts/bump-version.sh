#!/bin/bash
# 统一 bump 版本：usage: sh scripts/bump-version.sh 0.1.5
#
# 版本单一来源是根 Cargo.toml 的 [workspace.package] version：本脚本只改这一处，
# 再同步各 Node 包 / Tauri 配置 / Cargo.lock 里 workspace 成员的 version。
# 完成后调用 scripts/check-versions.mjs 复核（不一致会直接失败）。
set -e
V=$1
[ -z "$V" ] && echo "用法: sh scripts/bump-version.sh <新版本，如 0.1.5>" && exit 1
if ! printf '%s' "$V" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$'; then
  echo "bump-version: 版本号不合法：$V（期望形如 0.1.5）" >&2
  exit 1
fi

cd "$(dirname "$0")/.." || exit 1

# 1) 单一版本源：根 Cargo.toml 的 [workspace.package] version（只改该段里的 version 行）
perl -0pi -e 's/(\[workspace\.package\]\r?\n[ \t]*version[ \t]*=[ \t]*")[^"]*(")/${1}'"$V"'${2}/' Cargo.toml
SOURCE_VERSION=$(grep -A6 '^\[workspace\.package\]' Cargo.toml | grep -m1 '^version' | sed 's/.*"\(.*\)"/\1/' | tr -d '\r')
if [ "$SOURCE_VERSION" != "$V" ]; then
  echo "bump-version: 更新 Cargo.toml 的 [workspace.package] version 失败（当前 $SOURCE_VERSION）" >&2
  exit 1
fi

# 2) Node 包与 Tauri 配置里的顶层 version 字段
for f in package.json src-tauri/tauri.conf.json npm/package.json companion-ui/package.json; do
  perl -pi -e 's/^([ \t]*"version"[ \t]*:[ \t]*")[^"]*(")/${1}'"$V"'${2}/' "$f"
done
for f in npm/platform/*/package.json; do
  perl -pi -e 's/^([ \t]*"version"[ \t]*:[ \t]*")[^"]*(")/${1}'"$V"'${2}/' "$f"
done
# 主包 optionalDependencies 引用平台包的版本（key 一定是 workbuddy-shift-<platform>-<arch>）
perl -pi -e 's/("workbuddy-shift-[a-z0-9-]+"[ \t]*:[ \t]*")[^"]*(")/${1}'"$V"'${2}/g' npm/package.json

# 3) Cargo.lock：每个 workspace 成员一条 [[package]] version，必须一起升，
#    否则 --locked 构建会认为 lock 与 manifest 不一致。成员名从根 Cargo.toml 的
#    [workspace] members 推导（不依赖 cargo / 联网）。
WORKSPACE_MEMBER_DIRS=$(awk '
  /^\[workspace\]/ { in_ws = 1; next }
  in_ws && /^\[/ { in_ws = 0 }
  in_ws && /^[ \t]*members[ \t]*=[ \t]*\[/ { in_members = 1 }
  in_members { print }
  in_members && /\]/ { in_members = 0 }
' Cargo.toml | grep -o '"[^"]*"' | tr -d '"')
WORKSPACE_MEMBERS=""
for member_dir in $WORKSPACE_MEMBER_DIRS; do
  member_name=$(awk '/^\[package\]/{in_pkg=1; next} in_pkg && /^\[/{in_pkg=0} in_pkg' "$member_dir/Cargo.toml" \
    | grep -m1 '^name[ \t]*=' | sed 's/.*"\(.*\)".*/\1/' | tr -d '\r')
  if [ -z "$member_name" ]; then
    echo "bump-version: 无法从 $member_dir/Cargo.toml 读取 package.name" >&2
    exit 1
  fi
  WORKSPACE_MEMBERS="$WORKSPACE_MEMBERS $member_name"
done
if [ -z "$WORKSPACE_MEMBERS" ]; then
  echo "bump-version: 无法从 Cargo.toml 的 [workspace] members 推导 workspace 包名" >&2
  exit 1
fi

for member_name in $WORKSPACE_MEMBERS; do
  perl -0pi -e "s/(\[\[package\]\]\r?\nname = \"$member_name\"\r?\nversion = \")[^\"]*(\")/\${1}$V\${2}/g" Cargo.lock
done

# 4) 复核：所有版本来源一致（含 Cargo.lock 里的 workspace 成员）
node scripts/check-versions.mjs
echo "所有版本已同步为 $V（单一来源：Cargo.toml [workspace.package] version）"

#!/bin/bash
# 纯命令行打 dmg（不依赖 Finder/AppleScript，CI 无头环境可用）
# 用法：sh scripts/make-dmg.sh <版本> <arch> [.app 路径]
#   arch: aarch64 | x86_64
#   .app 路径默认 target/release/bundle/macos/WorkBuddy Shift.app（Tauri 2 按 productName 命名）
#   DMG 内含「应用程序」文件夹链接，拖入即安装到 /Applications
#
# dmgbuild writes the Finder layout and invokes hdiutil under the hood.  Do
# not fall back to a plain hdiutil image: that would silently lose the large
# icon positions and drag arrow this script is responsible for.
#
# Keep this wrapper POSIX-shell compatible: CI invokes it with `sh`, and macOS
# users commonly do the same when running the release script locally.
set -eu

usage() {
  echo "用法: sh scripts/make-dmg.sh <版本> <arch> [.app 路径]" >&2
}

if [ "$#" -lt 2 ] || [ "$#" -gt 3 ]; then
  usage
  exit 1
fi

V=$1
ARCH=$2
DEFAULT_APP="target/release/bundle/macos/WorkBuddy Shift.app"
APP=${3:-$DEFAULT_APP}

if [ -z "$V" ] || [ -z "$ARCH" ]; then
  usage
  exit 1
fi

case "$ARCH" in
  aarch64|x86_64) ;;
  *)
    echo "make-dmg: 不支持的架构：${ARCH}（只支持 aarch64 或 x86_64）" >&2
    exit 1
    ;;
esac

if ! command -v dmgbuild >/dev/null 2>&1; then
  echo "未找到 dmgbuild；请先安装固定版本的 dmgbuild（CI 会自动准备）" >&2
  exit 1
fi

if [ ! -d "$APP" ]; then
  echo "未找到 .app: $APP" >&2
  exit 1
fi

APP=$(cd "$APP" && pwd -P)
SCRIPT_DIR=$(dirname "$0")
SETTINGS_DIR=$(cd "$SCRIPT_DIR" && pwd -P)
SETTINGS="$SETTINGS_DIR/dmgbuild-settings.py"
if [ ! -f "$SETTINGS" ]; then
  echo "未找到 dmgbuild 设置文件: $SETTINGS" >&2
  exit 1
fi

OUT="workbuddy-shift_${V}_${ARCH}.dmg"
TMP_DIR=$(mktemp -d "${TMPDIR:-/tmp}/workbuddy-shift-dmg.XXXXXX")
TMP_OUT="$TMP_DIR/$OUT"
trap 'rm -rf "$TMP_DIR"' EXIT

echo "打 dmg: $APP → $OUT"
dmgbuild \
  -s "$SETTINGS" \
  -D "app=$APP" \
  "workbuddy-shift" \
  "$TMP_OUT"

# Publish atomically after dmgbuild has completed.  A failed build therefore
# cannot leave a partial DMG at the release path.
mv -f "$TMP_OUT" "$OUT"
echo "✓ 完成: $OUT ($(stat -f%z "$OUT") 字节)"

#!/bin/bash
# 生成 tauri updater 版本清单 latest-<os>-<arch>.json
# 用法：
#   UPDATE_OS=macos UPDATE_ARCH=aarch64 sh scripts/gen-update-json.sh [owner] [repo]
#   UPDATE_OS=windows UPDATE_ARCH=x86_64 sh scripts/gen-update-json.sh [owner] [repo]
#   UPDATE_OS=linux UPDATE_ARCH=x86_64 UPDATE_BUNDLE=appimage|deb sh scripts/gen-update-json.sh [owner] [repo]
# 可选：BUNDLE_DIR、UPDATE_ARCHIVE_NAME、UPDATE_KEYS、UPDATE_JSON_NAME
set -e
cd "$(dirname "$0")/.." || exit 1

OWNER="${1:-magicapple123}"
REPO="${2:-workbuddy-shift}"
# 版本单一来源：根 Cargo.toml 的 [workspace.package] version（调用方也可用 UPDATE_VERSION 覆盖）
VERSION="${UPDATE_VERSION:-$(grep -A6 '^\[workspace\.package\]' Cargo.toml | grep -m1 '^version' | sed 's/.*"\(.*\)"/\1/' | tr -d '\r')}"
if [ -z "$VERSION" ]; then
  echo "gen-update-json: 无法从 Cargo.toml 的 [workspace.package] version 读取版本号" >&2
  exit 1
fi
UPDATE_OS="${UPDATE_OS:-macos}"
UPDATE_ARCH="${UPDATE_ARCH:-aarch64}"
UPDATE_ARCHIVE_NAME="${UPDATE_ARCHIVE_NAME:-}"

case "$UPDATE_ARCH" in
  aarch64|x86_64) ;;
  *)
    echo "gen-update-json: 不支持的架构：${UPDATE_ARCH}（只支持 aarch64 或 x86_64）" >&2
    exit 1
    ;;
esac

case "$UPDATE_OS" in
  macos)
    DEFAULT_BUNDLE="macos"
    SIG_GLOB="*.app.tar.gz.sig"
    PLATFORM_KEYS="darwin-$UPDATE_ARCH"
    ;;
  windows)
    DEFAULT_BUNDLE="nsis"
    # Tauri 2 createUpdaterArtifacts=true 签的是当前版本安装包：
    # <productName>_<version>_x64-setup.exe.sig（productName 含空格，这里用 glob 匹配）
    SIG_GLOB="*_${VERSION}_x64-setup.exe.sig"
    PLATFORM_KEYS="windows-$UPDATE_ARCH-nsis windows-$UPDATE_ARCH"
    ;;
  linux)
    # 一次调用只处理一种安装形态：
    #   appimage → 签名 *.AppImage.sig，keys linux-<arch>-appimage + 兜底 linux-<arch>
    #   deb      → 签名 *.deb.sig，keys linux-<arch>-deb
    UPDATE_BUNDLE="${UPDATE_BUNDLE:-appimage}"
    case "$UPDATE_BUNDLE" in
      appimage)
        SIG_GLOB="*.AppImage.sig"
        PLATFORM_KEYS="linux-$UPDATE_ARCH-appimage linux-$UPDATE_ARCH"
        ;;
      deb)
        SIG_GLOB="*.deb.sig"
        PLATFORM_KEYS="linux-$UPDATE_ARCH-deb"
        ;;
      *)
        echo "gen-update-json: 不支持的 Linux 安装形态：${UPDATE_BUNDLE}（只支持 appimage 或 deb）" >&2
        exit 1
        ;;
    esac
    DEFAULT_BUNDLE="$UPDATE_BUNDLE"
    ;;
  *)
    echo "gen-update-json: 不支持的系统：${UPDATE_OS}（只支持 macos、windows 或 linux）" >&2
    exit 1
    ;;
esac

# UPDATE_KEYS 覆盖默认 key 列表，由调用方显式声明，避免依赖脚本内部默认值
if [ -n "${UPDATE_KEYS:-}" ]; then
  PLATFORM_KEYS="$UPDATE_KEYS"
fi

BUNDLE_DIR="${BUNDLE_DIR:-target/release/bundle/$DEFAULT_BUNDLE}"
if [ ! -d "$BUNDLE_DIR" ] && [ -d "src-tauri/target/release/bundle/$DEFAULT_BUNDLE" ]; then
  BUNDLE_DIR="src-tauri/target/release/bundle/$DEFAULT_BUNDLE"
fi

SIG_MATCHES=$(find "$BUNDLE_DIR" -maxdepth 2 -type f -name "$SIG_GLOB" | sort)
SIG_COUNT=$(printf '%s\n' "$SIG_MATCHES" | sed '/^$/d' | wc -l | tr -d ' ')
if [ "$SIG_COUNT" != 1 ]; then
  echo "gen-update-json: 期望恰好 1 个签名包（${SIG_GLOB}），实际 $SIG_COUNT" >&2
  echo "  命名示例：macOS *.app.tar.gz.sig；Windows *_${VERSION}_x64-setup.exe.sig；Linux *.AppImage.sig / *.deb.sig" >&2
  printf '%s\n' "$SIG_MATCHES" >&2
  ls -la "$BUNDLE_DIR" >&2 || true
  exit 1
fi
SIG_FILE=$SIG_MATCHES
ARCHIVE_FILE="${SIG_FILE%.sig}"
# UPDATE_JSON_NAME 覆盖输出文件名：linux 的 deb 条目用 latest-linux-deb-<arch>.json，避免与 appimage 条目互相覆盖
JSON_FILE="$BUNDLE_DIR/${UPDATE_JSON_NAME:-latest-$UPDATE_OS-$UPDATE_ARCH.json}"

if [ ! -f "$SIG_FILE" ] || [ ! -f "$ARCHIVE_FILE" ]; then
  echo "gen-update-json: 未找到签名更新包（$SIG_GLOB in ${BUNDLE_DIR}）" >&2
  ls -la "$BUNDLE_DIR" >&2 || true
  exit 1
fi

SIGNATURE=$(cat "$SIG_FILE")
if [ -n "$UPDATE_ARCHIVE_NAME" ]; then
  ARCHIVE_NAME="$UPDATE_ARCHIVE_NAME"
else
  ARCHIVE_NAME=$(basename "$ARCHIVE_FILE")
fi
PUB_DATE=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
# 第 4 条 B：资产 URL 必须是 tag 固定路径。用 releases/latest/download 时，
# 「检查到 vN」与「下载」之间若发布了 vN+1，会拿到 vN+1 的包却用 vN 的签名校验。
TAG="v${VERSION#v}"
URL="https://github.com/$OWNER/$REPO/releases/download/$TAG/$ARCHIVE_NAME"

# 第 4 条 H：清单 notes 从 delivery/release-notes-v<ver>.md 注入，弹窗可展示更新内容。
NOTES_JSON='""'
NOTES_FILE="docs/release-notes-v${VERSION}.md"
[ -f "$NOTES_FILE" ] || NOTES_FILE="delivery/release-notes-v${VERSION}.md"
if [ -f "$NOTES_FILE" ]; then
  if command -v python3 >/dev/null 2>&1; then
    NOTES_JSON=$(python3 -c 'import json,sys; print(json.dumps(open(sys.argv[1], encoding="utf-8").read()))' "$NOTES_FILE")
  elif command -v python >/dev/null 2>&1; then
    NOTES_JSON=$(python -c 'import json,sys; print(json.dumps(open(sys.argv[1], encoding="utf-8").read()))' "$NOTES_FILE")
  else
    echo "gen-update-json: 未找到 python，notes 留空" >&2
  fi
fi

PLATFORMS=""
for key in $PLATFORM_KEYS; do
  [ -n "$PLATFORMS" ] && PLATFORMS="$PLATFORMS,"
  PLATFORMS="$PLATFORMS
    \"$key\": {
      \"signature\": \"$SIGNATURE\",
      \"url\": \"$URL\"
    }"
done

mkdir -p "$BUNDLE_DIR"
cat > "$JSON_FILE" << EOF
{
  "version": "$VERSION",
  "notes": $NOTES_JSON,
  "pub_date": "$PUB_DATE",
  "platforms": {$PLATFORMS
  }
}
EOF
echo "gen-update-json: 已生成 $JSON_FILE"

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

# 第 4 条 H / R5-01：清单 `notes` 的语义是**卡片摘要（纯文本）**，不是 Release 说明全文。
#
# `docs/release-notes-v<ver>.md` 是面向 GitHub Release 页写的（安装包选择表、资产用途、
# 「下载哪个」这类段落只对 Release 页有意义），整份塞进应用内卡片会露出 `##`、`| --- |`、
# `> ` 之类的 Markdown 残留。完整说明继续由 `GITHUB_RELEASE_URL` 承载（卡片底部
# 「查看完整说明」），这里只投影出「能放进 360px 卡片」的摘要。
#
# 取值顺序：
#   1. release notes 里的 `<!-- card:begin -->` … `<!-- card:end -->` 区块（推荐，语义清晰）；
#   2. 没有标记时退化为正文：跳过标题行与 `发布日期：` 行，按行清洗整份说明。
# 清洗规则：表格整段丢弃，去行首标题 / 引用 / 列表编号标记，去 `**` `__` 反引号，
# `[文字](链接)` → 文字，连续空行压成一个，最终 ≤600 字符（超出按行边界截断并加省略号）。
NOTES_JSON='""'
NOTES_FILE="docs/release-notes-v${VERSION}.md"
[ -f "$NOTES_FILE" ] || NOTES_FILE="delivery/release-notes-v${VERSION}.md"

PYTHON_BIN=""
if command -v python3 >/dev/null 2>&1; then
  PYTHON_BIN="python3"
elif command -v python >/dev/null 2>&1; then
  PYTHON_BIN="python"
fi

# 读 $2 指向的 release notes，把卡片摘要以 JSON 字符串（带引号）打到 stdout。
clean_notes() {
  "$1" - "$2" <<'PY'
import json
import re
import sys

LIMIT = 600
CARD_BEGIN = "<!-- card:begin -->"
CARD_END = "<!-- card:end -->"

HEADING_RE = re.compile(r"^\s{0,3}#{1,6}\s*")
QUOTE_RE = re.compile(r"^\s{0,3}>\s*")
BULLET_RE = re.compile(r"^\s*[-*+]\s+")
ORDERED_RE = re.compile(r"^\s*\d+[.)]\s+")
COMMENT_RE = re.compile(r"<!--.*?-->")
LINK_RE = re.compile(r"!?\[([^\]]*)\]\([^)]*\)")
DATE_RE = re.compile(r"^发布日期\s*[:：]")


def read_text(path):
    with open(path, encoding="utf-8") as handle:
        return handle.read().replace("\r\n", "\n").replace("\r", "\n")


def extract_card(text):
    """取 `<!-- card:begin -->` … `<!-- card:end -->` 区块；没有标记返回 None。"""
    start = text.find(CARD_BEGIN)
    if start < 0:
        return None
    start += len(CARD_BEGIN)
    end = text.find(CARD_END, start)
    if end < 0:
        return None
    return text[start:end]


def is_separator(line):
    """表格分隔行：`| --- | --- |`（也接受 `--- | ---`）。"""
    stripped = line.strip()
    if "|" not in stripped or "---" not in stripped:
        return False
    return all(char in "-:| " for char in stripped)


def is_table_line(line):
    return is_separator(line) or line.count("|") >= 2


def drop_tables(lines):
    """整段丢弃表格：表头 + 分隔行 + 其后的数据行（遇空行结束）。"""
    kept = []
    index = 0
    while index < len(lines):
        line = lines[index]
        following = lines[index + 1] if index + 1 < len(lines) else ""
        if "|" in line and is_separator(following):
            index += 2
            while index < len(lines) and "|" in lines[index] and lines[index].strip():
                index += 1
            continue
        if is_table_line(line):
            index += 1
            continue
        kept.append(line)
        index += 1
    return kept


def clean_line(line):
    """单行清洗：去 HTML 注释 / 图片 / 链接 / 行首标记 / 行内强调标记。"""
    line = COMMENT_RE.sub("", line)
    line = LINK_RE.sub(r"\1", line)
    for pattern in (HEADING_RE, QUOTE_RE, BULLET_RE, ORDERED_RE):
        line = pattern.sub("", line)
    line = line.replace("**", "").replace("__", "").replace("`", "")
    return line.strip()


def to_lines(text):
    """把说明正文投影成「一行一句」的纯文本行；空白行压成一个。"""
    lines = []
    pending_blank = False
    for line in drop_tables(text.split("\n")):
        # 标题行整行丢弃：卡片自带「发现新版本 vX」标题，`## 新增` 这类分节标签
        # 在 360px 宽度里只会变成一行孤立的「新增」，没有信息量。
        if HEADING_RE.match(line):
            continue
        value = clean_line(line)
        if not value:
            pending_blank = bool(lines)
            continue
        if DATE_RE.match(value):
            continue
        if pending_blank:
            lines.append("")
            pending_blank = False
        lines.append(value)
    while lines and not lines[0]:
        lines.pop(0)
    while lines and not lines[-1]:
        lines.pop()
    return lines


def truncate(lines, limit=LIMIT):
    """按行边界截断到 limit 字符；被截掉内容时补一个省略号。"""
    kept = []
    used = 0
    for line in lines:
        extra = len(line) + (1 if kept else 0)
        if used + extra > limit:
            # 第一行本身就超限（没有换行的长段落）：只能按字符硬截断。
            if not kept:
                return line[: limit - 1] + "…"
            text = "\n".join(kept)
            if len(text) + 1 <= limit:
                return text + "…"
            return text[: limit - 1] + "…"
        kept.append(line)
        used += extra
    return "\n".join(kept)


def main():
    raw = read_text(sys.argv[1])
    body = extract_card(raw)
    if body is None:
        body = raw
    summary = truncate(to_lines(body))
    sys.stderr.write(
        "gen-update-json: notes 清洗 %d → %d 字符（上限 %d）\n"
        % (len(raw), len(summary), LIMIT)
    )
    print(json.dumps(summary))


main()
PY
}

if [ -f "$NOTES_FILE" ]; then
  if [ -n "$PYTHON_BIN" ]; then
    NOTES_JSON=$(clean_notes "$PYTHON_BIN" "$NOTES_FILE")
    [ -n "$NOTES_JSON" ] || NOTES_JSON='""'
  else
    # 无 python 时不猜摘要：宁可在卡片上只显示版本号与「查看完整说明」，也不要塞脏 Markdown。
    echo "gen-update-json: 未找到 python，notes 留空" >&2
  fi
else
  echo "gen-update-json: 未找到 $NOTES_FILE，notes 留空" >&2
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
# 清单是发布产物：写完立刻自检一次。notes 里的引号 / 反引号若把 JSON 写坏，
# 客户端会把它当成「清单不可达」而不是报错，发版后才排查代价极高。
if [ -n "$PYTHON_BIN" ]; then
  if ! "$PYTHON_BIN" -c 'import json,sys; json.load(open(sys.argv[1], encoding="utf-8"))' "$JSON_FILE" >/dev/null 2>&1; then
    echo "gen-update-json: 生成的清单不是合法 JSON：$JSON_FILE" >&2
    exit 1
  fi
fi
echo "gen-update-json: 已生成 $JSON_FILE"

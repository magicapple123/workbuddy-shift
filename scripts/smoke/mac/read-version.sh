#!/usr/bin/env bash
# 读 macOS `.app` 的版本与主程序路径，输出一行 JSON（macOS 侧的「read-version」helper）。
#
# 为什么单独一个脚本：`CFBundleShortVersionString` 在 `Contents/Info.plist` 里、主程序名在
# `CFBundleExecutable` 里——两者都要 PlistBuddy 读，塞进 driver 会让 driver 变长且难复用。
#
# 用法：
#   bash scripts/smoke/mac/read-version.sh --app "/Applications/WorkBuddy Shift.app"
#   → {"app":"...","shortVersion":"0.2.5","buildVersion":"0.2.5","executable":"workbuddy-shift",
#      "binary":"/Applications/WorkBuddy Shift.app/Contents/MacOS/workbuddy-shift","ok":true,"error":""}
#
# 约定：
# - **永远 exit 0**：判定交给 driver（读不出来就是 `ok=false` + `error` 文本）；
# - 不猜版本号：`Info.plist` 缺 `CFBundleShortVersionString` 时 `shortVersion` 留空。

set -eu

APP=""
while [ $# -gt 0 ]; do
  case "$1" in
    --app)
      APP=${2:-}
      shift 2
      ;;
    *)
      # 位置参数也接受（`read-version.sh "/Applications/x.app"`）
      if [ -z "${APP}" ]; then APP=$1; fi
      shift
      ;;
  esac
done

# JSON 字符串转义（路径里理论上不会有引号，但不要让一个引号把整份 JSON 弄坏）。
json_str() {
  printf '%s' "${1:-}" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/\t/ /g' | tr -d '\r\n'
}

emit() {
  # $1=ok(true/false) $2=short $3=build $4=exe $5=binary $6=error
  printf '{"app":"%s","shortVersion":"%s","buildVersion":"%s","executable":"%s","binary":"%s","ok":%s,"error":"%s"}\n' \
    "$(json_str "${APP}")" "$(json_str "${2:-}")" "$(json_str "${3:-}")" "$(json_str "${4:-}")" \
    "$(json_str "${5:-}")" "${1}" "$(json_str "${6:-}")"
}

if [ -z "${APP}" ]; then
  emit false "" "" "" "" "缺少 --app 参数"
  exit 0
fi

INFO_PLIST="${APP}/Contents/Info.plist"
if [ ! -f "${INFO_PLIST}" ]; then
  emit false "" "" "" "" "Info.plist 不存在：${INFO_PLIST}"
  exit 0
fi

PLIST_BUDDY="/usr/libexec/PlistBuddy"
SHORT=""
BUILD=""
EXE_NAME=""
if [ -x "${PLIST_BUDDY}" ]; then
  SHORT=$("${PLIST_BUDDY}" -c 'Print :CFBundleShortVersionString' "${INFO_PLIST}" 2>/dev/null || true)
  BUILD=$("${PLIST_BUDDY}" -c 'Print :CFBundleVersion' "${INFO_PLIST}" 2>/dev/null || true)
  EXE_NAME=$("${PLIST_BUDDY}" -c 'Print :CFBundleExecutable' "${INFO_PLIST}" 2>/dev/null || true)
fi

if [ -z "${SHORT}" ]; then
  emit false "${SHORT}" "${BUILD}" "${EXE_NAME}" "" "读不到 CFBundleShortVersionString（或 PlistBuddy 不可用）"
  exit 0
fi

BINARY=""
if [ -n "${EXE_NAME}" ] && [ -x "${APP}/Contents/MacOS/${EXE_NAME}" ]; then
  BINARY="${APP}/Contents/MacOS/${EXE_NAME}"
else
  # 兜底：Contents/MacOS 下第一个可执行文件（CFBundleExecutable 与实际文件名不一致时）。
  for candidate in "${APP}/Contents/MacOS/"*; do
    if [ -f "${candidate}" ] && [ -x "${candidate}" ]; then
      BINARY="${candidate}"
      EXE_NAME=$(basename "${candidate}")
      break
    fi
  done
fi

if [ -z "${BINARY}" ]; then
  emit false "${SHORT}" "${BUILD}" "${EXE_NAME}" "" "Contents/MacOS 下没有可执行文件"
  exit 0
fi

emit true "${SHORT}" "${BUILD}" "${EXE_NAME}" "${BINARY}" ""
exit 0

#!/usr/bin/env bash
# 读 Linux deb 形态的包名 / 已安装版本 / 主程序路径，输出一行 JSON。
#
# 两种用法：
#   bash scripts/smoke/linux/read-version.sh --deb workbuddy-shift-linux-x86_64.deb
#       → 从**包元数据**读 `Package` / `Version`（安装前的期望值；包名不靠猜）
#   bash scripts/smoke/linux/read-version.sh --package workbuddy-shift
#       → 从 `dpkg-query` 读已安装版本，并用 `dpkg -L` 找 `/usr/bin/` 下的主程序
#
# 输出：`{"package":"...","version":"...","binary":"...","installed":true|false,"ok":true|false,"error":""}`
#
# 约定：
# - **永远 exit 0**：读不出来就是 `ok=false` + `error`，判定交给 driver；
# - 包名一律从 deb 元数据或 `dpkg-query` 拿，**不写死**：写死的包名一旦改名，所有版本
#   断言都会变成「读不到 = 失败」，掩盖真实缺陷。

set -eu

MODE=""
TARGET=""
while [ $# -gt 0 ]; do
  case "$1" in
    --deb)
      MODE="deb"
      TARGET=${2:-}
      shift 2
      ;;
    --package)
      MODE="package"
      TARGET=${2:-}
      shift 2
      ;;
    *)
      if [ -z "${MODE}" ]; then
        # 位置参数：.deb 结尾按文件处理，否则按包名处理
        case "$1" in
          *.deb) MODE="deb" ;;
          *) MODE="package" ;;
        esac
        TARGET=$1
      fi
      shift
      ;;
  esac
done

json_str() {
  printf '%s' "${1:-}" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/\t/ /g' | tr -d '\r\n'
}

emit() {
  # $1=package $2=version $3=binary $4=installed(true/false) $5=ok(true/false) $6=error
  printf '{"package":"%s","version":"%s","binary":"%s","installed":%s,"ok":%s,"error":"%s"}\n' \
    "$(json_str "${1:-}")" "$(json_str "${2:-}")" "$(json_str "${3:-}")" \
    "${4:-false}" "${5:-false}" "$(json_str "${6:-}")"
}

if [ -z "${MODE}" ]; then
  emit "" "" "" false false "缺少 --deb / --package 参数"
  exit 0
fi

PKG=""
VERSION=""
BINARY=""
INSTALLED="false"
ERROR=""

if [ "${MODE}" = "deb" ]; then
  if [ ! -f "${TARGET}" ]; then
    emit "" "" "" false false "deb 文件不存在：${TARGET}"
    exit 0
  fi
  if ! command -v dpkg-deb >/dev/null 2>&1; then
    emit "" "" "" false false "缺少 dpkg-deb（非 Debian 系环境）"
    exit 0
  fi
  PKG=$(dpkg-deb -f "${TARGET}" Package 2>/dev/null || true)
  VERSION=$(dpkg-deb -f "${TARGET}" Version 2>/dev/null || true)
  INSTALLED="false"
  OK="true"
  if [ -z "${PKG}" ] || [ -z "${VERSION}" ]; then
    OK="false"
    ERROR="dpkg-deb 读不到 Package/Version"
  fi
  emit "${PKG}" "${VERSION}" "" "${INSTALLED}" "${OK}" "${ERROR}"
  exit 0
fi

# --package：读已安装版本并发现主程序
PKG=${TARGET}
if ! command -v dpkg-query >/dev/null 2>&1; then
  emit "${PKG}" "" "" false false "缺少 dpkg-query"
  exit 0
fi
VERSION=$(dpkg-query -W -f='${Version}' "${PKG}" 2>/dev/null || true)
if [ -n "${VERSION}" ]; then
  INSTALLED="true"
else
  ERROR="包未安装或 dpkg-query 读不到版本：${PKG}"
fi
# 主程序：/usr/bin 下的第一个（Tauri deb 把二进制装到 /usr/bin）。
BINARY=$(dpkg -L "${PKG}" 2>/dev/null | grep '^/usr/bin/' | head -n 1 || true)
if [ -z "${BINARY}" ] && [ "${INSTALLED}" = "true" ]; then
  ERROR="${ERROR}（已安装但 /usr/bin 下没有条目）"
fi

OK="true"
if [ "${INSTALLED}" != "true" ]; then
  OK="false"
fi
emit "${PKG}" "${VERSION}" "${BINARY}" "${INSTALLED}" "${OK}" "${ERROR}"
exit 0

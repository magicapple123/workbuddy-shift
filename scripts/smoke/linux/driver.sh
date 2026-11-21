#!/usr/bin/env bash
# Linux 桌面形态的发版后验收驱动（设计 §5.3 的 L0–L8；`ubuntu-latest`）。
#
# 两条升级路径都要验，因为它们对应两类真实用户：
#   1. **deb**：`dpkg -i 新版本` 覆盖老版本（`pkg-upgrade`），证据是 `dpkg-query` 的版本变化；
#   2. **AppImage**：同路径替换 AppImage（`appimage-replace`），证据是 sha256 == Release 资产
#      digest + 能起来。
#
# 硬约定（设计 §2.2 / §6.1 / §9.2）：
# - **本脚本恒 exit 0**：断言写进 `common.py cell`，判定交给 Gate 步骤；异常也要落成 fail 格；
# - 托管 runner 没有桌面会话，`xvfb-run` 下 WebKitGTK 可能起不来。按设计 §6.2 三条件判
#   `blocked`(J)（进程 20s 内退出 + 沙箱目录完全没创建 + 命中环境特征），否则判 `fail`(F)；
# - `WB_SHIFT_HOME` 指向新的临时空目录，不碰真实用户数据；
# - 包名/主程序名一律从 deb 元数据与 `dpkg -L` 读，不写死。
#
# 用法：
#   bash scripts/smoke/linux/driver.sh --version 0.2.5 --prev 0.2.4
set -eu

VERSION=""
PREV=""
OUT=""
REPO=""

usage() {
  cat <<'USAGE'
用法：bash scripts/smoke/linux/driver.sh --version <当前版本> [--prev <上一版>] [--out <结果JSON>] [--repo owner/repo]

  --version  目标版本（0.2.5，可带 v 前缀），必填
  --prev     上一版（0.2.4）；缺省或为空时升级格记 skipped
  --out      结果 JSON 路径（默认 <repo>/artifacts/release-smoke/linux.json）
  --repo     owner/repo（默认读 GH_REPO / GITHUB_REPOSITORY）

本脚本恒 exit 0；判定由 Gate 步骤（render-conclusion.py --gate）负责。
USAGE
}

while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION=${2:-}; shift 2 ;;
    --prev | --prev-version) PREV=${2:-}; shift 2 ;;
    --out) OUT=${2:-}; shift 2 ;;
    --repo) REPO=${2:-}; shift 2 ;;
    -h | --help) usage; exit 0 ;;
    *) printf '::warning::未知参数：%s\n' "$1" >&2; exit 0 ;;
  esac
done

if [ -z "${VERSION}" ]; then
  printf '::error::缺少 --version（driver 不写结果文件 → Gate 会报「结果文件不存在」）\n' >&2
  exit 0
fi
VERSION=$(printf '%s' "${VERSION}" | sed -e 's/^[vV]//' -e 's/^ *//' -e 's/ *$//')
PREV=$(printf '%s' "${PREV}" | sed -e 's/^[vV]//' -e 's/^ *//' -e 's/ *$//')
if [ -z "${REPO}" ]; then
  REPO=${GH_REPO:-${GITHUB_REPOSITORY:-}}
fi

DRIVER_DIR=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH='' cd -- "${DRIVER_DIR}/../../.." && pwd)
case "${OUT}" in
  "") OUT="${REPO_ROOT}/artifacts/release-smoke/linux.json" ;;
esac
case "${OUT}" in
  /*) ;;
  *) OUT="$(pwd)/${OUT}" ;;
esac
SMOKE_RESULT_FILE=${OUT}
export SMOKE_RESULT_FILE

# shellcheck disable=SC1091
. "${DRIVER_DIR}/../common.sh"

# L0a：建结果文件。**必须在写第一格之前**——Gate 读不到结果文件会判「驱动脚本没落盘」
# （流水线缺陷），而不是「某一格失败」。顶层口径 `pkg-upgrade` 是 deb 主线；
# AppImage 那几格用**格子级** `appimage-replace` 覆盖（设计 §9.1 的 `upgradeMode?`）。
smoke_emit linux "${VERSION}" "${PREV}" pkg-upgrade

WORK=$(smoke_workdir)
# 证据落在**结果 JSON 旁边**（`artifacts/release-smoke/evidence/linux/`）：workflow 上传的是
# 仓库内的 `path: artifacts/`，放 RUNNER_TEMP 的 startup.log 从来没进过 artifact——格子里写
# 「见 startup.log」而 artifact 里只有 JSON（run 44941972411 的现场）。
EVID=$(smoke_evidence_dir linux)
APPIMAGE_URL_TRACE="${EVID}/appimage.txt"

SMOKE_FINISHED=""
finish_and_exit() {
  if [ -n "${SMOKE_FINISHED}" ]; then
    return 0
  fi
  SMOKE_FINISHED=1
  smoke_finish || true
  smoke_log "linux driver 结束：结果 ${SMOKE_RESULT_FILE} · 证据 ${EVID}"
  exit 0
}
trap finish_and_exit EXIT

# ---------------------------------------------------------------------------
# 小工具
# ---------------------------------------------------------------------------

json_get() {
  printf '%s' "${1}" | "${SMOKE_PYTHON}" -c '
import json, sys
key = sys.argv[1]
try:
    data = json.load(sys.stdin)
except Exception:
    print("")
    raise SystemExit
value = data.get(key) if isinstance(data, dict) else None
print("" if value is None else value)
' "${2}"
}

gh_download() {
  mkdir -p "${3}"
  if [ -n "${REPO}" ]; then
    gh release download "${1}" --pattern "${2}" --dir "${3}" --clobber --repo "${REPO}"
  else
    gh release download "${1}" --pattern "${2}" --dir "${3}" --clobber
  fi
}

release_digest() {
  if [ -n "${REPO}" ]; then
    _rd_view=$(gh release view "${1}" --json assets --repo "${REPO}" 2>/dev/null || true)
  else
    _rd_view=$(gh release view "${1}" --json assets 2>/dev/null || true)
  fi
  if [ -z "${_rd_view}" ]; then
    printf '\n'
    return 0
  fi
  printf '%s' "${_rd_view}" | "${SMOKE_PYTHON}" -c '
import json, sys
name = sys.argv[1]
try:
    data = json.load(sys.stdin)
except Exception:
    print("")
    raise SystemExit
for asset in data.get("assets") or []:
    if asset.get("name") == name:
        print(asset.get("digest") or "")
        raise SystemExit
print("")
' "${2}" || printf '\n'
}

read_deb_json() {
  bash "${DRIVER_DIR}/read-version.sh" --deb "${1}" 2>/dev/null || printf '{"ok":false,"error":"read-version.sh --deb 失败"}'
}

read_installed_json() {
  bash "${DRIVER_DIR}/read-version.sh" --package "${1}" 2>/dev/null || printf '{"ok":false,"error":"read-version.sh --package 失败"}'
}

version_matches() {
  # deb 版本可能带 debian revision：0.2.4 / 0.2.4-1 都算匹配
  case "${1:-}" in
    "${2}" | "${2}"-*) return 0 ;;
    *) return 1 ;;
  esac
}

kill_app() {
  pkill -f "${1}" >/dev/null 2>&1 || true
  sleep 1
}

# ---------------------------------------------------------------------------
# L0：系统依赖（apt，超时重试一次）
# ---------------------------------------------------------------------------
smoke_group "L0 系统依赖"
APT_PACKAGES="xvfb libwebkit2gtk-4.1-0 libgtk-3-0"
apt_install() {
  sudo apt-get install -y --no-install-recommends ${APT_PACKAGES} >"${EVID}/apt.log" 2>&1
}
if smoke_soft "apt-get update（可失败，runner 常已就绪）" sudo apt-get update -qq; then :; fi
if apt_install || apt_install; then
  smoke_pass install "系统依赖就绪（${APT_PACKAGES}）"
else
  _missing=""
  for _pkg in ${APT_PACKAGES}; do
    dpkg -s "${_pkg}" >/dev/null 2>&1 || _missing="${_missing} ${_pkg}"
  done
  if [ -n "${_missing}" ]; then
    smoke_fail install J "装齐系统依赖（${APT_PACKAGES}）" "缺失：${_missing}；见 apt.log"
  else
    smoke_pass install "系统依赖就绪（${APT_PACKAGES}，apt 报错但包已在）"
  fi
fi
# 可选：托盘图标库。缺它 dpkg 的依赖检查可能失败，但不算验收失败。
sudo apt-get install -y --no-install-recommends libayatana-appindicator3-1 >"${EVID}/apt-appindicator.log" 2>&1 || true
smoke_endgroup

# ---------------------------------------------------------------------------
# L1：取资产（deb ×2 + AppImage；deb 两版同名，必须分目录）
# ---------------------------------------------------------------------------
DEB_CURR=""
DEB_PREV=""
APPIMAGE=""
APPIMAGE_PREV=""

smoke_group "L1 取资产"
if gh_download "v${VERSION}" '*linux-x86_64.deb' "${WORK}/pkg/curr" >"${EVID}/gh-curr-deb.log" 2>&1; then
  DEB_CURR=$(find "${WORK}/pkg/curr" -name '*.deb' -print -quit 2>/dev/null || true)
fi
if [ -n "${DEB_CURR}" ]; then
  smoke_pass install "取到当前版本 deb（$(basename "${DEB_CURR}")）"
  # 不再把 deb 本体拷进 evidence：那是 ~16MB 的二进制，和「证据」无关，只会撑大 artifact。
  printf 'file=%s\nsha256=%s\n' "$(basename "${DEB_CURR}")" "$(smoke_sha256 "${DEB_CURR}" 2>/dev/null || echo '读不到')" \
    >"${EVID}/deb-curr.txt" 2>/dev/null || true
else
  smoke_fail install D "取到当前版本 deb（v${VERSION}）" "gh release download 失败或产物为空，见 gh-curr-deb.log"
fi

if [ -n "${PREV}" ]; then
  if gh_download "v${PREV}" '*linux-x86_64.deb' "${WORK}/pkg/prev" >"${EVID}/gh-prev-deb.log" 2>&1; then
    DEB_PREV=$(find "${WORK}/pkg/prev" -name '*.deb' -print -quit 2>/dev/null || true)
  fi
  if [ -n "${DEB_PREV}" ]; then
    smoke_pass install "取到上一版 deb（$(basename "${DEB_PREV}")）"
  else
    smoke_fail install D "取到上一版 deb（v${PREV}）" "gh release download 失败或产物为空，见 gh-prev-deb.log"
  fi
else
  smoke_skip install "上一版 deb" "没有上一版（首次发版）"
fi

if gh_download "v${VERSION}" '*linux-x86_64.AppImage' "${WORK}/pkg/appimage" >"${EVID}/gh-appimage.log" 2>&1; then
  APPIMAGE=$(find "${WORK}/pkg/appimage" -name '*.AppImage' -print -quit 2>/dev/null || true)
fi
if [ -n "${APPIMAGE}" ]; then
  chmod +x "${APPIMAGE}" 2>/dev/null || true
  smoke_pass install "取到当前版本 AppImage（$(basename "${APPIMAGE}")）" "" appimage-replace
else
  smoke_skip install "取到 AppImage（v${VERSION}）" "gh release download 失败或产物为空，见 gh-appimage.log" appimage-replace
fi

# 上一版 AppImage 只用于「同路径替换」格（appimage-replace 的升级证据），拿不到不阻断
# 其它格：那一格会自己判 D 类（Release 缺资产）。
if [ -n "${PREV}" ]; then
  if gh_download "v${PREV}" '*linux-x86_64.AppImage' "${WORK}/pkg/appimage-prev" >"${EVID}/gh-appimage-prev.log" 2>&1; then
    APPIMAGE_PREV=$(find "${WORK}/pkg/appimage-prev" -name '*.AppImage' -print -quit 2>/dev/null || true)
  fi
fi
smoke_endgroup

# ---------------------------------------------------------------------------
# L2–L3：装上一版 deb → 断言版本
# ---------------------------------------------------------------------------
PKG_NAME=""

smoke_group "L2–L3 安装上一版 deb"
if [ -z "${DEB_PREV}" ]; then
  smoke_skip install "dpkg -i 安装上一版" "没有上一版 deb"
  smoke_skip version "上一版版本 == ${PREV:-—}" "没有上一版"
else
  _meta=$(read_deb_json "${DEB_PREV}")
  PKG_NAME=$(json_get "${_meta}" package)
  _debdep=$(json_get "${_meta}" version)
  if [ -z "${PKG_NAME}" ]; then
    smoke_fail install E "从 deb 元数据读出包名" "$(json_get "${_meta}" error)"
  else
    smoke_log "包名=${PKG_NAME} deb 版本=${_debdep}"
    if smoke_soft "dpkg -i ${DEB_PREV}" sudo dpkg -i "${DEB_PREV}"; then
      smoke_pass install "dpkg -i 安装 ${PKG_NAME}（上一版）"
    else
      # 依赖不全时 dpkg 会半途失败：修一次依赖再判，避免把「缺库」误判成「安装器坏了」
      smoke_soft "apt-get -f install（修依赖）" sudo apt-get -f install -y
      _fixed=""
      if sudo dpkg -i "${DEB_PREV}" >"${EVID}/dpkg-prev-retry.log" 2>&1; then _fixed="retry"; fi
      if [ -n "${_fixed}" ]; then
        smoke_pass install "dpkg -i 安装 ${PKG_NAME}（上一版，修依赖后成功）"
      else
        smoke_fail install E "dpkg -i 安装 ${PKG_NAME}（上一版）" "exit=${SMOKE_LAST_STATUS}，见 dpkg-prev-retry.log"
      fi
    fi

    _json=$(read_installed_json "${PKG_NAME}")
    printf '%s\n' "${_json}" >"${EVID}/dpkg-prev.json"
    _ver=$(json_get "${_json}" version)
    if [ -z "${_ver}" ]; then
      smoke_fail version H "装后 dpkg-query 能读到版本（期望 ${PREV}）" "$(json_get "${_json}" error)"
    elif ! version_matches "${_ver}" "${PREV}"; then
      smoke_fail version H "上一版 dpkg-query 版本 == ${PREV}" "实际 ${_ver}"
    else
      smoke_pass version "上一版 dpkg-query 版本 == ${PREV}（实际 ${_ver}）"
    fi
  fi
fi
smoke_endgroup

# ---------------------------------------------------------------------------
# L4–L5：装当前版本 deb（升级）→ 断言版本变化
# ---------------------------------------------------------------------------
smoke_group "L4–L5 升级到当前版本"
if [ -z "${DEB_CURR}" ] || [ -z "${PKG_NAME}" ]; then
  if [ -z "${DEB_CURR}" ]; then
    smoke_skip upgrade "dpkg -i 升级到 ${VERSION}" "当前版本 deb 缺失"
    smoke_skip version "升级后版本 == ${VERSION}" "当前版本 deb 缺失"
  else
    smoke_fail upgrade E "dpkg -i 升级到 ${VERSION}" "包名未知（上一版安装格已失败）"
    smoke_skip version "升级后版本 == ${VERSION}" "包名未知"
  fi
elif [ -z "${PREV}" ]; then
  smoke_skip upgrade "dpkg -i 升级（无上一版）" "首次发版，升级格不适用"
  if sudo dpkg -i "${DEB_CURR}" >"${EVID}/dpkg-curr.log" 2>&1; then
    smoke_pass install "dpkg -i 安装 ${PKG_NAME}（首次安装）"
  else
    smoke_fail install E "dpkg -i 安装 ${PKG_NAME}" "见 dpkg-curr.log"
  fi
  _json=$(read_installed_json "${PKG_NAME}")
  _ver=$(json_get "${_json}" version)
  if version_matches "${_ver}" "${VERSION}"; then
    smoke_pass version "安装后 dpkg-query 版本 == ${VERSION}（实际 ${_ver}）"
  else
    smoke_fail version H "安装后 dpkg-query 版本 == ${VERSION}" "实际 ${_ver:-读不到}"
  fi
else
  if sudo dpkg -i "${DEB_CURR}" >"${EVID}/dpkg-curr.log" 2>&1; then
    smoke_pass upgrade "dpkg -i 从 ${PREV} 升级到 ${VERSION}"
  else
    smoke_fail upgrade E "dpkg -i 从 ${PREV} 升级到 ${VERSION}" "见 dpkg-curr.log"
  fi
  _json=$(read_installed_json "${PKG_NAME}")
  printf '%s\n' "${_json}" >"${EVID}/dpkg-curr.json"
  _ver=$(json_get "${_json}" version)
  BIN_PATH=$(json_get "${_json}" binary)
  if ! version_matches "${_ver}" "${VERSION}"; then
    smoke_fail version H "升级后 dpkg-query 版本 == ${VERSION}" "实际 ${_ver:-读不到}"
  else
    smoke_pass version "升级后 dpkg-query 版本 == ${VERSION}（实际 ${_ver}）"
  fi
fi
smoke_endgroup

# 无论走哪条分支，都用「装完后」的读数取主程序路径（首次发版分支也要）。
if [ -n "${PKG_NAME}" ] && [ -z "${BIN_PATH:-}" ]; then
  _json=$(read_installed_json "${PKG_NAME}")
  BIN_PATH=$(json_get "${_json}" binary)
fi

# 静态保护（run 44941972411 的教训）：deb 里有 `companion-runtime`（悬浮窗 sidecar）与
# `workbuddy-shift-desktop`（桌面壳）两个可执行文件。若 read-version.sh 选中的名字里带
# `companion`，说明**driver 选错了验证对象**——那不是「启动即崩」，把它记成 F 会让整条
# 流水线去开一张假 issue。所以在这里判死，并让启动格一律 blocked（原因指回这一格）。
#
# 类别选 E 而不是 A：A 会触发 `gh run rerun --failed`，而选错主程序重跑一万次也一样错；
# E 直接落到 issue，让人来看 driver。detail 里写清「driver 缺陷」，不许伪装成产品失败。
DRIVER_BIN_BAD=""
if [ -n "${BIN_PATH:-}" ] && printf '%s' "$(basename "${BIN_PATH}")" | grep -qi 'companion'; then
  DRIVER_BIN_BAD="yes"
  smoke_fail install E "装后主程序应为桌面壳（basename 不含 companion）" \
    "driver 缺陷（不是产品缺陷）：read-version.sh 选中了 ${BIN_PATH}——它是悬浮窗 sidecar，不是 Tauri 桌面壳。重跑不会修好，请修 scripts/smoke/linux/read-version.sh 的选择规则（.desktop 的 Exec → /usr/bin 下排除 companion-*）。启动格因此记 blocked，不判产品失败。"
  BIN_PATH=""
fi

if [ -n "${BIN_PATH:-}" ]; then
  smoke_log "主程序：${BIN_PATH}"
  printf 'binary=%s\n' "${BIN_PATH}" >"${EVID}/binary.txt" 2>/dev/null || true
fi

# ---------------------------------------------------------------------------
# L6：启动（xvfb-run，20s 存活）+ 沙箱落盘
# ---------------------------------------------------------------------------
SANDBOX_STARTUP=""

smoke_group "L6 启动（xvfb）"
if [ -z "${BIN_PATH:-}" ] || [ ! -x "${BIN_PATH}" ]; then
  if [ -n "${DRIVER_BIN_BAD}" ]; then
    smoke_blocked startup "xvfb 下启动后 20s 内存活" \
      "driver 缺陷：主程序被识别成 companion-runtime（见 install 格的 ❌），本格不判产品失败；修 read-version.sh 后再验"
  else
    smoke_blocked startup "xvfb 下启动后 20s 内存活" "没有可执行的主程序（安装格已失败）：${BIN_PATH:-未发现}"
  fi
else
  kill_app "$(basename "${BIN_PATH}")"
  SANDBOX_STARTUP=$(smoke_sandbox)
  xvfb-run -a --server-args='-screen 0 1280x1024x24' \
    env WB_SHIFT_HOME="${SANDBOX_STARTUP}" "${BIN_PATH}" --hidden \
    >"${EVID}/startup.log" 2>&1 &
  XPID=$!
  smoke_log "启动 pid=${XPID}（xvfb-run 包装）binary=${BIN_PATH}"
  sleep 20
  _wrapper_alive="no"
  _app_alive="no"
  if kill -0 "${XPID}" 2>/dev/null; then _wrapper_alive="yes"; fi
  if pgrep -f "$(basename "${BIN_PATH}")" >/dev/null 2>&1; then _app_alive="yes"; fi
  if [ "${_wrapper_alive}" = "yes" ] || [ "${_app_alive}" = "yes" ]; then
    smoke_pass startup "xvfb 下启动后 20s 内进程存活（wrapper=${_wrapper_alive} app=${_app_alive}）"
    echo "alive" >"${WORK}/startup.alive"
  else
    _store="${SANDBOX_STARTUP}/.workbuddy-shift"
    _store_exists="no"
    if [ -d "${_store}" ]; then _store_exists="yes"; fi
    _webkit="no"
    if ldconfig -p 2>/dev/null | grep -q 'libwebkit2gtk-4.1'; then _webkit="yes"; fi
    _display_err="no"
    if grep -qi 'cannot open display\|Failed to create WebKitWebProcess' "${EVID}/startup.log" 2>/dev/null; then _display_err="yes"; fi
    _detail="20s 内已退出 · store 存在=${_store_exists} · libwebkit2gtk-4.1=${_webkit} · display/WebKit 报错=${_display_err} · 见 startup.log"
    if [ "${_store_exists}" = "no" ] && { [ "${_webkit}" = "no" ] || [ "${_display_err}" = "yes" ]; }; then
      smoke_blocked startup "xvfb 下启动后 20s 内存活" "${_detail}（设计 §6.2 判 J）"
    else
      smoke_fail startup F "xvfb 下启动后 20s 内进程存活" "${_detail}"
    fi
  fi
fi
smoke_endgroup

smoke_group "L6b 沙箱落盘"
if [ -z "${SANDBOX_STARTUP}" ] || [ ! -d "${SANDBOX_STARTUP}/.workbuddy-shift" ]; then
  smoke_skip startup "<沙箱>/.workbuddy-shift 落盘" "沙箱目录不存在（启动格已判定）"
else
  ERROR_LOG="${SANDBOX_STARTUP}/.workbuddy-shift/error.log"
  if [ ! -f "${ERROR_LOG}" ]; then
    smoke_pass startup "沙箱落盘且 error.log 无 crash 行（本次未产生 error.log）"
  else
    _bad=$("${SMOKE_PYTHON}" -c '
import json, sys
bad = []
lines = 0
for raw in open(sys.argv[1], encoding="utf-8", errors="replace"):
    text = raw.strip()
    if not text:
        continue
    lines += 1
    try:
        entry = json.loads(text)
    except Exception:
        bad.append("第 %d 行不是合法 JSON" % lines)
        continue
    kind = entry.get("kind")
    if kind in ("frontend_crash", "frontend_unhandled", "panic"):
        bad.append("第 %d 行 kind=%s" % (lines, kind))
    version = entry.get("appVersion")
    if version and str(version) != sys.argv[2]:
        bad.append("第 %d 行 appVersion=%s" % (lines, version))
print("; ".join(bad) if bad else "OK(lines=%d)" % lines)
' "${ERROR_LOG}" "${VERSION}")
    cp "${ERROR_LOG}" "${EVID}/error-log.txt" 2>/dev/null || true
    if [ "${_bad#OK}" != "${_bad}" ]; then
      smoke_pass startup "沙箱落盘 + error.log 无 crash 行（${_bad}）"
    else
      smoke_fail startup F "error.log 无 crash 行且 appVersion 正确" "${_bad}"
    fi
  fi
fi
smoke_endgroup

# ---------------------------------------------------------------------------
# L7：AppImage 形态（sha256 == Release digest + 能起来）
# ---------------------------------------------------------------------------
smoke_group "L7 AppImage"
if [ -z "${APPIMAGE}" ]; then
  smoke_skip install "AppImage sha256 == Release digest" "没取到 AppImage" appimage-replace
  smoke_skip upgrade "AppImage 同路径替换（${PREV:-—} → ${VERSION}）" "没取到当前版本 AppImage" appimage-replace
  smoke_skip startup "AppImage 启动 20s 存活" "没取到 AppImage" appimage-replace
else
  _name=$(basename "${APPIMAGE}")
  _digest=$(release_digest "v${VERSION}" "${_name}")
  _local=$(smoke_sha256 "${APPIMAGE}")
  {
    printf 'file=%s\nlocal=%s\nrelease=%s\n' "${_name}" "${_local}" "${_digest}"
  } >"${APPIMAGE_URL_TRACE}"
  if [ -n "${_digest}" ] && [ "${_digest}" != "sha256:${_local}" ]; then
    smoke_fail install H "AppImage sha256 == Release 资产 digest（${_name}）" "local=${_local} release=${_digest}" appimage-replace
  else
    smoke_pass install "AppImage sha256 == ${_digest:-（Release 无 digest，跳过比对）}" "" appimage-replace
  fi

  # L7a：AppImage 形态的「升级」= 把新 AppImage **覆盖到同一路径**（用户的真实路径在这里
  # 用 `${WORK}/pkg/user/WorkBuddyShift.AppImage` 模拟）。证据是覆盖后 sha256 == Release
  # digest，且与上一版的 sha256 不同——「文件被替换了」是可机读的事实，不靠猜。
  if [ -z "${PREV}" ]; then
    smoke_skip upgrade "AppImage 同路径替换（— → ${VERSION}）" "没有上一版（首次发版）" appimage-replace
  elif [ -z "${APPIMAGE_PREV}" ]; then
    smoke_fail upgrade D "取到上一版 AppImage（v${PREV}）" "gh release download 失败或产物为空，见 gh-appimage-prev.log" appimage-replace
  else
    _user_dir="${WORK}/pkg/user"
    _user_app="${_user_dir}/WorkBuddyShift.AppImage"
    mkdir -p "${_user_dir}"
    cp "${APPIMAGE_PREV}" "${_user_app}"
    chmod +x "${_user_app}" 2>/dev/null || true
    _prev_sha=$(smoke_sha256 "${_user_app}")
    cp -f "${APPIMAGE}" "${_user_app}"
    chmod +x "${_user_app}" 2>/dev/null || true
    _new_sha=$(smoke_sha256 "${_user_app}")
    printf 'path=%s\nprev=%s\nnew=%s\nrelease=%s\n' "${_user_app}" "${_prev_sha}" "${_new_sha}" "${_digest}" \
      >"${EVID}/appimage-replace.txt"
    if [ "${_prev_sha}" = "${_new_sha}" ]; then
      smoke_fail upgrade H "AppImage 覆盖到同一路径后 sha256 变化（${PREV} → ${VERSION}）" \
        "覆盖前后 sha256 相同=${_new_sha}（下载到的两版其实是同一份文件？）" appimage-replace
    elif [ -n "${_digest}" ] && [ "${_digest}" != "sha256:${_new_sha}" ]; then
      smoke_fail upgrade H "覆盖后 AppImage sha256 == Release 资产 digest" \
        "local=${_new_sha} release=${_digest}" appimage-replace
    else
      smoke_pass upgrade "AppImage 同路径替换 ${PREV} → ${VERSION}（sha256 变化并等于 Release digest）" \
        "prev=${_prev_sha} new=${_new_sha}" appimage-replace
    fi
  fi

  # 先收掉 deb 形态的进程，否则 AppImage 的存活断言会被它冒充。
  kill_app 'workbuddy-shift'
  _app_sandbox=$(smoke_sandbox)
  # --appimage-extract-and-run：runner 里没有 FUSE，必须用解压模式（设计 L7）。
  xvfb-run -a --server-args='-screen 0 1280x1024x24' \
    env WB_SHIFT_HOME="${_app_sandbox}" APPIMAGE_EXTRACT_AND_RUN=1 \
    "${APPIMAGE}" --appimage-extract-and-run --hidden \
    >"${EVID}/appimage-run.log" 2>&1 &
  APID=$!
  smoke_log "AppImage 启动 pid=${APID}"
  sleep 20
  _alive="no"
  if kill -0 "${APID}" 2>/dev/null; then _alive="yes"; fi
  if pgrep -f 'workbuddy-shift' >/dev/null 2>&1; then _alive="yes"; fi
  if [ "${_alive}" = "yes" ]; then
    smoke_pass startup "AppImage（--appimage-extract-and-run）启动后 20s 内存活" "" appimage-replace
  else
    _store="$([ -d "${_app_sandbox}/.workbuddy-shift" ] && echo yes || echo no)"
    smoke_fail startup F "AppImage 启动后 20s 内进程存活" "store 存在=${_store}，见 appimage-run.log" appimage-replace
  fi
  kill_app 'workbuddy-shift' || true
  kill -9 "${APID}" >/dev/null 2>&1 || true
fi
smoke_endgroup

# ---------------------------------------------------------------------------
# L8：清理
# ---------------------------------------------------------------------------
smoke_group "L8 清理"
# 注意：只在 XPID 有值时 kill。`kill -9 0` 会杀掉**整个进程组**（连同 driver 自己，
# 甚至 CI 的日志收集进程），所以绝不能写成 `${XPID:-0}`。
if [ -n "${XPID:-}" ]; then
  kill -9 "${XPID}" >/dev/null 2>&1 || true
fi
kill_app 'workbuddy-shift'
pkill -f 'Xvfb' >/dev/null 2>&1 || true
smoke_log "清理完成"
smoke_endgroup

exit 0

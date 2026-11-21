#!/usr/bin/env bash
# macOS 桌面形态的发版后验收驱动（设计 §5.2 的 M1–M10；`macos-latest` = arm64）。
#
# 与 Windows 的区别：macOS 的**真实升级途径就是「用新 dmg 覆盖同一个 .app」**（设计 §0.4 的
# `pkg-upgrade`），没有应用内自更新链路，所以「升级」的证据就是 `CFBundleShortVersionString`
# 从 0.2.4 变成 0.2.5（版本号变化本身即证据）。
#
# 硬约定（设计 §2.2 / §9.2）：
# - **本脚本恒 exit 0**：每个断言通过 `common.py cell` 落盘，判定交给 workflow 的 Gate 步骤；
# - 任何异常都要变成 `fail` 格子（用 EXIT trap 兜底 finish，脚本自身崩掉也要留结果）；
# - 启动证据是四元的：20s 存活 / 第二实例退出 / `instance.lock` 落盘 / `error.log` 无 crash；
# - `WB_SHIFT_HOME` 每次指向新的临时空目录，不碰真实用户数据；
# - `xattr -dr com.apple.quarantine` **只在 runner 内**做（本期无公证，见设计 M3 与 R3）。
#
# x86_64 dmg 只做「资产存在 + 版本号可读」并记 `skipped`（设计 D1：x86_64 非目标）。
#
# 用法：
#   bash scripts/smoke/mac/driver.sh --version 0.2.5 --prev 0.2.4
#   bash scripts/smoke/mac/driver.sh --version 0.2.5 --prev 0.2.4 --out /tmp/macos.json
set -eu

VERSION=""
PREV=""
OUT=""
REPO=""

usage() {
  cat <<'USAGE'
用法：bash scripts/smoke/mac/driver.sh --version <当前版本> [--prev <上一版>] [--out <结果JSON>] [--repo owner/repo]

  --version  目标版本（0.2.5，可带 v 前缀），必填
  --prev     上一版（0.2.4）；缺省或为空时升级格记 skipped
  --out      结果 JSON 路径（默认 <repo>/artifacts/release-smoke/macos.json）
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
  "") OUT="${REPO_ROOT}/artifacts/release-smoke/macos.json" ;;
esac
case "${OUT}" in
  /*) ;;
  *) OUT="$(pwd)/${OUT}" ;;
esac
SMOKE_RESULT_FILE=${OUT}
export SMOKE_RESULT_FILE

# shellcheck disable=SC1091
. "${DRIVER_DIR}/../common.sh"

WORK=$(smoke_workdir)
# 证据落在结果 JSON 旁边（`artifacts/release-smoke/evidence/macos/`）：workflow 上传的是
# 仓库内的 `path: artifacts/`，放 RUNNER_TEMP 的日志不会被上传（格子里写「见 xxx.log」，
# artifact 里却只有 JSON）。
EVID=$(smoke_evidence_dir macos)
APP_NAME="WorkBuddy Shift.app"
APP_PATH=""
BINARY=""

SMOKE_FINISHED=""
finish_and_exit() {
  if [ -n "${SMOKE_FINISHED}" ]; then
    return 0
  fi
  SMOKE_FINISHED=1
  smoke_finish || true
  smoke_log "macos driver 结束：结果 ${SMOKE_RESULT_FILE} · 证据 ${EVID}"
  # 恒 exit 0：判定在 Gate（设计 §2.2）。
  exit 0
}
trap finish_and_exit EXIT

# ---------------------------------------------------------------------------
# 小工具
# ---------------------------------------------------------------------------

json_get() {
  # $1=json 文本 $2=字段名；取不到输出空行（不报错）
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
  # $1=tag $2=pattern $3=dir
  mkdir -p "${3}"
  if [ -n "${REPO}" ]; then
    gh release download "${1}" --pattern "${2}" --dir "${3}" --clobber --repo "${REPO}"
  else
    gh release download "${1}" --pattern "${2}" --dir "${3}" --clobber
  fi
}

release_digest() {
  # $1=tag $2=文件名 → 输出 "sha256:..." 或空行
  _rd_tag=$1
  _rd_name=$2
  if [ -n "${REPO}" ]; then
    _rd_view=$(gh release view "${_rd_tag}" --json assets --repo "${REPO}" 2>/dev/null || true)
  else
    _rd_view=$(gh release view "${_rd_tag}" --json assets 2>/dev/null || true)
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
' "${_rd_name}" || printf '\n'
}

read_app_json() {
  # $1=.app 路径 → read-version.sh 的 JSON（脚本自身恒 exit 0）
  bash "${DRIVER_DIR}/read-version.sh" --app "${1}" 2>/dev/null || printf '{"ok":false,"error":"read-version.sh 执行失败"}'
}

mount_dmg() {
  # $1=dmg $2=mountpoint → 0 成功
  mkdir -p "${2}"
  hdiutil attach -nobrowse -readonly -mountpoint "${2}" "${1}" >/dev/null 2>&1
}

detach_dmg() {
  hdiutil detach "${1}" >/dev/null 2>&1 || hdiutil detach -force "${1}" >/dev/null 2>&1 || true
}

install_app_from_dmg() {
  # $1=dmg → 拷贝到 /Applications 并去掉 quarantine；输出 .app 的绝对路径
  _ia_dmg=$1
  _ia_mount=$(mktemp -d "${WORK}/mnt.XXXXXX")
  if ! mount_dmg "${_ia_dmg}" "${_ia_mount}"; then
    return 1
  fi
  _ia_src=$(find "${_ia_mount}" -maxdepth 1 -name '*.app' -print -quit 2>/dev/null || true)
  if [ -z "${_ia_src}" ]; then
    detach_dmg "${_ia_mount}"
    return 1
  fi
  APP_NAME=$(basename "${_ia_src}")
  rm -rf "/Applications/${APP_NAME}" 2>/dev/null || sudo rm -rf "/Applications/${APP_NAME}" 2>/dev/null || true
  if ! cp -R "${_ia_src}" /Applications/ 2>/dev/null; then
    sudo cp -R "${_ia_src}" /Applications/ || {
      detach_dmg "${_ia_mount}"
      return 1
    }
  fi
  detach_dmg "${_ia_mount}"
  APP_PATH="/Applications/${APP_NAME}"
  # 只在 runner 内去隔离（设计 M3 / Q2）：不是绕过产品的签名校验，是让未公证的 .app 能跑。
  xattr -dr com.apple.quarantine "${APP_PATH}" >/dev/null 2>&1 || true
  printf '%s\n' "${APP_PATH}"
}

kill_all_app() {
  pkill -f "MacOS/${APP_NAME%.app}" >/dev/null 2>&1 || true
  pkill -f "${APP_NAME}" >/dev/null 2>&1 || true
  sleep 1
}

# ---------------------------------------------------------------------------
# M0：建结果
# ---------------------------------------------------------------------------
smoke_emit macos "${VERSION}" "${PREV}" pkg-upgrade

smoke_group "环境"
smoke_log "$(uname -a)"
smoke_log "arch=$(uname -m) version=${VERSION} prev=${PREV:-—} workdir=${WORK}"
if [ "$(uname -m)" != "arm64" ]; then
  smoke_warn "runner 不是 arm64（$(uname -m)）：aarch64 dmg 可能装不起来，相关格会如实失败"
fi
smoke_endgroup

# ---------------------------------------------------------------------------
# M1：取资产（aarch64 两个 + x86_64 两个）并校验 sha256
# ---------------------------------------------------------------------------
AARCH64_CURR=""
AARCH64_PREV=""
X86_CURR=""

smoke_group "M1 取资产"
if gh_download "v${VERSION}" '*_aarch64.dmg' "${WORK}/dmg/curr-aarch64" >"${EVID}/gh-curr-aarch64.log" 2>&1; then
  AARCH64_CURR=$(find "${WORK}/dmg/curr-aarch64" -name '*_aarch64.dmg' -print -quit 2>/dev/null || true)
fi
if [ -n "${AARCH64_CURR}" ]; then
  _name=$(basename "${AARCH64_CURR}")
  _digest=$(release_digest "v${VERSION}" "${_name}")
  _local=$(smoke_sha256 "${AARCH64_CURR}")
  {
    printf 'file=%s\nlocal=%s\nrelease=%s\n' "${_name}" "${_local}" "${_digest}"
  } >"${EVID}/sha256-curr-aarch64.txt"
  if [ -n "${_digest}" ] && [ "${_digest}" != "sha256:${_local}" ]; then
    smoke_fail install D "当前版本 aarch64 dmg sha256 与 Release digest 一致（${_name}）" \
      "local=${_local} release=${_digest}"
  else
    smoke_pass install "取到 ${_name}，sha256 ${_digest:-（Release 无 digest，跳过比对）}"
  fi
else
  smoke_fail install D "取到当前版本 aarch64 dmg（v${VERSION}）" "gh release download 失败或产物为空，见 gh-curr-aarch64.log"
fi

if [ -n "${PREV}" ]; then
  if gh_download "v${PREV}" '*_aarch64.dmg' "${WORK}/dmg/prev-aarch64" >"${EVID}/gh-prev-aarch64.log" 2>&1; then
    AARCH64_PREV=$(find "${WORK}/dmg/prev-aarch64" -name '*_aarch64.dmg' -print -quit 2>/dev/null || true)
  fi
  if [ -n "${AARCH64_PREV}" ]; then
    _name=$(basename "${AARCH64_PREV}")
    _digest=$(release_digest "v${PREV}" "${_name}")
    _local=$(smoke_sha256 "${AARCH64_PREV}")
    {
      printf 'file=%s\nlocal=%s\nrelease=%s\n' "${_name}" "${_local}" "${_digest}"
    } >"${EVID}/sha256-prev-aarch64.txt"
    if [ -n "${_digest}" ] && [ "${_digest}" != "sha256:${_local}" ]; then
      smoke_fail install D "上一版 aarch64 dmg sha256 与 Release digest 一致（${_name}）" \
        "local=${_local} release=${_digest}"
    else
      smoke_pass install "取到 ${_name}，sha256 ${_digest:-（Release 无 digest，跳过比对）}"
    fi
  else
    smoke_fail install D "取到上一版 aarch64 dmg（v${PREV}）" "gh release download 失败或产物为空，见 gh-prev-aarch64.log"
  fi
else
  smoke_skip install "上一版 aarch64 dmg" "没有上一版（首次发版）"
fi
smoke_endgroup

# ---------------------------------------------------------------------------
# M2–M4：装上一版 → 读版本（双读：PlistBuddy 版本 + 主程序存在）
# ---------------------------------------------------------------------------
PREV_INSTALLED=""

smoke_group "M2–M4 安装上一版"
if [ -z "${AARCH64_PREV}" ]; then
  smoke_skip install "安装上一版（dmg 覆盖）" "没有上一版 dmg"
  smoke_skip version "上一版版本号 == ${PREV:-—}" "没有上一版"
else
  if ! installed=$(install_app_from_dmg "${AARCH64_PREV}"); then
    smoke_fail install E "上一版 dmg 挂载并拷贝到 /Applications" "hdiutil attach 或 cp -R 失败"
  else
    smoke_pass install "上一版 dmg 装到 ${installed}"
    _json=$(read_app_json "${installed}")
    printf '%s\n' "${_json}" >"${EVID}/version-prev.json"
    _ok=$(json_get "${_json}" ok)
    _short=$(json_get "${_json}" shortVersion)
    _binary=$(json_get "${_json}" binary)
    if [ "${_ok}" != "True" ] && [ "${_ok}" != "true" ]; then
      smoke_fail version H "读上一版 CFBundleShortVersionString" "$(json_get "${_json}" error)"
    elif [ "${_short}" != "${PREV}" ]; then
      smoke_fail version H "上一版 CFBundleShortVersionString == ${PREV}" "实际 ${_short}"
    elif [ ! -x "${_binary}" ]; then
      smoke_fail install E "上一版主程序可执行" "binary=${_binary}"
    else
      PREV_INSTALLED=${installed}
      smoke_pass version "上一版 CFBundleShortVersionString == ${PREV}（binary=${_binary}）"
    fi
  fi
fi
smoke_endgroup

# ---------------------------------------------------------------------------
# M5–M6：覆盖升级为当前版本 → 版本号必须变化
# ---------------------------------------------------------------------------
smoke_group "M5–M6 覆盖升级"
if [ -z "${AARCH64_CURR}" ]; then
  smoke_skip upgrade "覆盖升级 ${PREV:-—} → ${VERSION}" "当前版本 dmg 缺失"
  smoke_skip version "升级后版本 == ${VERSION}" "当前版本 dmg 缺失"
elif [ -z "${PREV}" ]; then
  # 首次发版：直接装当前版本，为启动验收准备对象（升级格如实 skipped）。
  smoke_skip upgrade "覆盖升级（无上一版）" "首次发版，升级格不适用"
  if installed=$(install_app_from_dmg "${AARCH64_CURR}"); then
    APP_PATH=${installed}
    smoke_pass install "当前版本 dmg 装到 ${installed}"
  else
    smoke_fail install E "当前版本 dmg 挂载并拷贝到 /Applications" "hdiutil attach 或 cp -R 失败"
  fi
  smoke_skip version "覆盖升级后的版本变化" "首次发版，无升级动作"
else
  if ! installed=$(install_app_from_dmg "${AARCH64_CURR}"); then
    smoke_fail upgrade E "新 dmg 覆盖同一个 .app（${PREV} → ${VERSION}）" "hdiutil attach 或 cp -R 失败"
  else
    _json=$(read_app_json "${installed}")
    printf '%s\n' "${_json}" >"${EVID}/version-curr.json"
    _short=$(json_get "${_json}" shortVersion)
    _binary=$(json_get "${_json}" binary)
    if [ "${_short}" != "${VERSION}" ]; then
      smoke_fail upgrade H "覆盖升级后 CFBundleShortVersionString == ${VERSION}" "实际 ${_short}"
      smoke_fail version H "升级后版本 == ${VERSION}" "实际 ${_short}"
    else
      smoke_pass upgrade "新 dmg 覆盖同一个 .app（${PREV} → ${_short}）"
      smoke_pass version "升级后 CFBundleShortVersionString == ${VERSION}（binary=${_binary}）"
      APP_PATH=${installed}
    fi
  fi
fi
smoke_endgroup

# ---------------------------------------------------------------------------
# M7–M9：启动的四元证据
# ---------------------------------------------------------------------------
STARTUP_PID=""
SANDBOX_STARTUP=""

smoke_group "M7 启动"
if [ -z "${APP_PATH}" ] || [ ! -d "${APP_PATH}" ]; then
  smoke_blocked startup "启动后 20s 内存活" "没有可启动的 .app（安装/升级格已失败）"
else
  kill_all_app
  _json=$(read_app_json "${APP_PATH}")
  BINARY=$(json_get "${_json}" binary)
  SANDBOX_STARTUP=$(smoke_sandbox)
  if [ -z "${BINARY}" ] || [ ! -x "${BINARY}" ]; then
    smoke_fail startup F "启动前能定位到可执行文件" "binary=${BINARY}"
  else
    WB_SHIFT_HOME="${SANDBOX_STARTUP}" "${BINARY}" --hidden >"${EVID}/startup.log" 2>&1 &
    STARTUP_PID=$!
    smoke_log "启动 pid=${STARTUP_PID} binary=${BINARY} sandbox=${SANDBOX_STARTUP}"
    sleep 20
    if kill -0 "${STARTUP_PID}" 2>/dev/null; then
      smoke_pass startup "启动后 20s 内进程存活（pid=${STARTUP_PID}）"
    else
      _store="${SANDBOX_STARTUP}/.workbuddy-shift"
      wait "${STARTUP_PID}" 2>/dev/null && _code=0 || _code=$?
      _writable="yes"
      if [ ! -w "/Applications" ]; then _writable="no"; fi
      _detail="20s 内已退出（exit=${_code}）· store 存在=$([ -d "${_store}" ] && echo yes || echo no) · /Applications 可写=${_writable}"
      if [ ! -d "${_store}" ] && [ "${_writable}" = "no" ]; then
        smoke_blocked startup "启动后 20s 内存活" "${_detail}（设计 §6.2 判 J）"
      else
        smoke_fail startup F "启动后 20s 内进程存活" "${_detail}"
      fi
      STARTUP_PID=""
    fi
  fi
fi
smoke_endgroup

smoke_group "M8 单实例 + 落盘"
if [ -z "${STARTUP_PID}" ] || ! kill -0 "${STARTUP_PID}" 2>/dev/null; then
  smoke_skip startup "第二实例 15s 内退出" "首实例没起来，无从验证"
  smoke_skip startup "instance.lock 落盘" "首实例没起来，无从验证"
else
  WB_SHIFT_HOME="${SANDBOX_STARTUP}" "${BINARY}" --hidden >"${EVID}/startup-second.log" 2>&1 &
  _pid2=$!
  _deadline=$(( $(date +%s) + 15 ))
  while kill -0 "${_pid2}" 2>/dev/null && [ "$(date +%s)" -lt "${_deadline}" ]; do
    sleep 1
  done
  if kill -0 "${_pid2}" 2>/dev/null; then
    kill -9 "${_pid2}" >/dev/null 2>&1 || true
    smoke_fail startup F "第二实例 15s 内退出（单实例生效）" "第二实例仍在运行，已强杀"
  else
    wait "${_pid2}" 2>/dev/null && _code2=0 || _code2=$?
    smoke_pass startup "第二实例 15s 内退出（单实例生效，exit=${_code2}）"
  fi

  LOCK="${SANDBOX_STARTUP}/.workbuddy-shift/instance.lock"
  if [ -e "${LOCK}" ]; then
    smoke_pass startup "instance.lock 已落盘（${LOCK}）"
  else
    smoke_fail startup F "instance.lock 已落盘（${LOCK}）" "目录内容：$(ls -a "${SANDBOX_STARTUP}/.workbuddy-shift" 2>/dev/null | tr '\n' ' ' || echo '目录不存在')"
  fi
fi
smoke_endgroup

smoke_group "M9 error.log 负向断言"
if [ -z "${SANDBOX_STARTUP}" ] || [ ! -d "${SANDBOX_STARTUP}/.workbuddy-shift" ]; then
  smoke_skip startup "error.log 无 crash 行" "沙箱目录不存在（启动格已判定）"
else
  ERROR_LOG="${SANDBOX_STARTUP}/.workbuddy-shift/error.log"
  if [ ! -f "${ERROR_LOG}" ]; then
    smoke_pass startup "error.log 无 crash 行（本次未产生 error.log）"
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
    if [ "${_bad}" = "OK" ] || [ "${_bad#OK}" != "${_bad}" ]; then
      smoke_pass startup "error.log 无 crash 行（${_bad}，appVersion=${VERSION}）"
    else
      smoke_fail startup F "error.log 无 crash 行且 appVersion 正确" "${_bad}"
    fi
  fi
fi
smoke_endgroup

# ---------------------------------------------------------------------------
# M10：x86_64 dmg（非目标，只验资产存在 + 版本可读）→ skipped
# ---------------------------------------------------------------------------
smoke_group "M10 x86_64 dmg（非目标，记 skipped）"
for _tag_version in "${PREV}" "${VERSION}"; do
  if [ -z "${_tag_version}" ]; then
    continue
  fi
  _dir="${WORK}/dmg/x86-${_tag_version}"
  if ! gh_download "v${_tag_version}" '*_x86_64.dmg' "${_dir}" >"${EVID}/gh-x86-${_tag_version}.log" 2>&1; then
    smoke_skip install "x86_64 dmg（v${_tag_version}）资产存在" "下载失败（非目标，不影响结论）"
    continue
  fi
  _dmg=$(find "${_dir}" -name '*_x86_64.dmg' -print -quit 2>/dev/null || true)
  if [ -z "${_dmg}" ]; then
    smoke_skip install "x86_64 dmg（v${_tag_version}）资产存在" "产物为空（非目标，不影响结论）"
    continue
  fi
  _mount=$(mktemp -d "${WORK}/mnt-x86.XXXXXX")
  _short=""
  if mount_dmg "${_dmg}" "${_mount}"; then
    _src=$(find "${_mount}" -maxdepth 1 -name '*.app' -print -quit 2>/dev/null || true)
    if [ -n "${_src}" ]; then
      _json=$(read_app_json "${_src}")
      _short=$(json_get "${_json}" shortVersion)
    fi
    detach_dmg "${_mount}"
  fi
  printf '%s %s\n' "$(basename "${_dmg}")" "${_short}" >>"${EVID}/x86-versions.txt"
  smoke_skip install "x86_64 dmg（v${_tag_version}）资产存在 + 版本可读：${_short:-读不到}" \
    "x86_64 非目标（设计 D1），只记录不判定"
done
smoke_endgroup

# ---------------------------------------------------------------------------
# 清理
# ---------------------------------------------------------------------------
smoke_group "清理"
if [ -n "${STARTUP_PID}" ]; then
  kill -TERM "${STARTUP_PID}" >/dev/null 2>&1 || true
  sleep 2
  kill -9 "${STARTUP_PID}" >/dev/null 2>&1 || true
fi
kill_all_app
smoke_log "剩余进程：$(ps ax | grep -c '[W]orkBuddy Shift' || true)"
smoke_endgroup

# EXIT trap 负责 finish + exit 0。
exit 0

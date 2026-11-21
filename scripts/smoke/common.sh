#!/usr/bin/env bash
# release-smoke 的 bash 侧薄封装：日志前缀 / 失败即停 / 临时目录 / 超时包装。
#
# 目的：让每个平台的 driver 长得一样，并且**不在 shell 里重复定义结果字段名**。
# 所有结果 JSON 的读写都转发给 `scripts/smoke/common.py`（唯一真源，见设计 §9.2）。
#
# 用法（driver 第一行）：
#     . "$(dirname "$0")/../common.sh"
#
# 约定：
# - 本文件被 source 时设置 `set -eu`：未定义变量、命令失败都立即停下；
# - 「允许失败」的命令用 `smoke_soft <描述> <命令...>`，它把成败记进变量而不中断；
# - 断言失败**不退出**，而是 `smoke_fail <action> <category> <assertion>` 记一格再继续，
#   这样一格失败不会让矩阵缺格（设计 §2.2）；
# - 脚本结尾必须 `smoke_finish`，否则 CI 的 Gate 读不到完整结果。
#
# 只依赖 POSIX sh + python3；`timeout` 缺失（macOS）时自动退回内置实现。

# shellcheck shell=bash

set -eu

# ---------------------------------------------------------------------------
# 路径
# ---------------------------------------------------------------------------

# 本文件可能被 `scripts/smoke/<form>/driver.sh` source（例如 `npm/driver.sh`），
# 此时 `$0` 是 **driver** 的路径、不是本文件的路径——按 `$0` 推目录会把 `scripts/smoke/npm`
# 当成 `SMOKE_DIR`，`SMOKE_COMMON_PY` 随之指错（契约模块找不到，所有格子都写不出来）。
# bash 下 `${BASH_SOURCE}` 指向被 source 的文件本身；POSIX sh 没有该变量时退回 `$0`。
_smoke_self=${BASH_SOURCE:-$0}
SMOKE_DIR=$(CDPATH='' cd -- "$(dirname -- "${_smoke_self}")" && pwd)
case "${SMOKE_DIR}" in
  */scripts/smoke) SMOKE_REPO_ROOT=$(CDPATH='' cd -- "${SMOKE_DIR}/../.." && pwd) ;;
  *) SMOKE_REPO_ROOT=$(CDPATH='' cd -- "${SMOKE_DIR}/.." && pwd) ;;
esac
SMOKE_COMMON_PY="${SMOKE_DIR}/common.py"
SMOKE_PYTHON=$(command -v python3 || command -v python || true)
: "${SMOKE_RESULT_DIR:=${SMOKE_REPO_ROOT}/artifacts/release-smoke}"
: "${SMOKE_ARTIFACT_DIR:=${SMOKE_REPO_ROOT}/artifacts}"

export SMOKE_DIR SMOKE_REPO_ROOT SMOKE_COMMON_PY SMOKE_PYTHON
export SMOKE_RESULT_DIR SMOKE_ARTIFACT_DIR

# ---------------------------------------------------------------------------
# 日志（GitHub Actions 的分组与注解；本地跑时是普通文本）
# ---------------------------------------------------------------------------

smoke_log() {
  printf '[smoke] %s\n' "$*"
}

smoke_warn() {
  printf '::warning::%s\n' "$*" >&2 || printf 'WARN %s\n' "$*" >&2
}

smoke_error() {
  printf '::error::%s\n' "$*" >&2 || printf 'ERROR %s\n' "$*" >&2
}

smoke_group() {
  # $1 = 分组标题
  printf '::group::%s\n' "$1"
}

smoke_endgroup() {
  printf '::endgroup::\n'
}

# 需要外部工具时用这个检查，缺了就记一格 skipped 而不是崩掉。
smoke_require() {
  if ! command -v "$1" >/dev/null 2>&1; then
    smoke_error "缺少必需命令：$1（${2:-该格无法执行}）"
    return 1
  fi
  return 0
}

smoke_require_python() {
  if [ -z "${SMOKE_PYTHON}" ]; then
    smoke_error "缺少 python3：结果 JSON 无法写出（流水线缺陷，不是产品问题）"
    return 1
  fi
  return 0
}

# ---------------------------------------------------------------------------
# 超时包装
# ---------------------------------------------------------------------------

# 有 GNU timeout 就用它；macOS 默认没有，退回 `&` + 轮询。
smoke_timeout() {
  _st_secs=$1
  shift
  if command -v timeout >/dev/null 2>&1; then
    timeout "${_st_secs}" "$@"
    return $?
  fi
  "$@" &
  _st_pid=$!
  _st_waited=0
  while kill -0 "${_st_pid}" 2>/dev/null; do
    if [ "${_st_waited}" -ge "${_st_secs}" ]; then
      kill -TERM "${_st_pid}" 2>/dev/null || true
      sleep 1
      kill -KILL "${_st_pid}" 2>/dev/null || true
      wait "${_st_pid}" 2>/dev/null || true
      return 124
    fi
    sleep 1
    _st_waited=$((_st_waited + 1))
  done
  wait "${_st_pid}"
}

# ---------------------------------------------------------------------------
# 临时目录 / 沙箱
# ---------------------------------------------------------------------------

# 每个 driver 一份工作目录；退出时清理（CI 里保不保留由 artifacts 决定）。
smoke_workdir() {
  _sw_base=${RUNNER_TEMP:-${TMPDIR:-/tmp}}
  _sw_dir=$(mktemp -d "${_sw_base}/release-smoke.XXXXXX")
  printf '%s\n' "${_sw_dir}"
}

# 证据落点：`<结果 JSON 所在目录>/evidence/<form>`。
#
# 为什么不再放 `RUNNER_TEMP`：workflow 上传的是仓库内的 `path: artifacts/`，临时目录里的
# `startup.log` / `volume-write.log` 从来没被上传过——格子的 detail 写着「见 xxx.log」，
# 而 artifact 里只有一份 JSON（run 44941972411 的现场）。证据必须和结果**同源**，否则
# 「有证据」这句话在结论里就是假的。
smoke_evidence_dir() {
  # $1 = 形态（windows / macos / linux / npm / docker）
  _sed_form=${1:-}
  if [ -z "${_sed_form}" ]; then
    smoke_error "smoke_evidence_dir 需要形态名（如 linux）"
    return 1
  fi
  if [ -n "${SMOKE_RESULT_FILE:-}" ]; then
    _sed_base=$(dirname -- "${SMOKE_RESULT_FILE}")
  else
    _sed_base=${SMOKE_RESULT_DIR}
  fi
  _sed_dir="${_sed_base}/evidence/${_sed_form}"
  mkdir -p "${_sed_dir}"
  printf '%s\n' "${_sed_dir}"
}

# 桌面三端统一用 WB_SHIFT_HOME 指向这个空目录：账号库 / 配置 / 日志全落进来，
# 不碰真实用户数据（PRD §3 的口径，产品既有能力）。
smoke_sandbox() {
  _ss_dir=$(smoke_workdir)
  export WB_SHIFT_HOME="${_ss_dir}"
  # 日志走 stderr：本函数的 stdout 是**返回值**（driver 用 `dir=$(smoke_sandbox)` 接），
  # 混进一行日志会让 WB_SHIFT_HOME 变成多行字符串——沙箱失效，而且很难看出来。
  smoke_log "沙箱 WB_SHIFT_HOME=${WB_SHIFT_HOME}" >&2
  printf '%s\n' "${_ss_dir}"
}

# ---------------------------------------------------------------------------
# 结果 JSON（一律转发 common.py，shell 不定义任何字段名）
# ---------------------------------------------------------------------------

smoke_emit() {
  # $1=form $2=version [$3=prev_version] [$4=upgrade_mode]
  [ -n "${SMOKE_RESULT_FILE:-}" ] || {
    smoke_error "smoke_emit 需要先设置 SMOKE_RESULT_FILE"
    return 1
  }
  smoke_require_python || return 1
  _se_args="--form $1 --version $2 --out ${SMOKE_RESULT_FILE}"
  [ $# -ge 3 ] && [ -n "${3:-}" ] && _se_args="${_se_args} --prev-version $3"
  [ $# -ge 4 ] && [ -n "${4:-}" ] && _se_args="${_se_args} --upgrade-mode $4"
  # shellcheck disable=SC2086  # 参数已在上面拆好，不能加引号
  "${SMOKE_PYTHON}" "${SMOKE_COMMON_PY}" emit ${_se_args}
}

smoke_cell() {
  # $1=action $2=status $3=assertion [$4=category] [$5=detail] [$6=upgrade_mode]
  #
  # $6 是**格子级**升级口径覆盖（设计 §9.1 的 `upgradeMode?`）：只有该格与形态主线不同时
  # 才传（Linux 的 AppImage 格传 `appimage-replace`，形态主线是 `pkg-upgrade`）。
  # 词表校验在 common.py（唯一真源），这里只负责转参数。
  #
  # 用位置参数重建命令行而不是拼字符串：detail 里常有空格与分号，拼串会被 word splitting
  # 拆散（POSIX sh 没有数组），症状是「断言原文被截断」，很难查。
  smoke_require_python || return 1
  _sc_cat=${4:-}
  _sc_mode=${6:-}
  set -- --file "${SMOKE_RESULT_FILE}" --action "$1" --status "$2" --assertion "$3" \
    --detail "${5:-}"
  if [ -n "${_sc_cat}" ]; then
    set -- "$@" --category "${_sc_cat}"
  fi
  if [ -n "${_sc_mode}" ]; then
    set -- "$@" --upgrade-mode "${_sc_mode}"
  fi
  "${SMOKE_PYTHON}" "${SMOKE_COMMON_PY}" cell "$@"
}

smoke_pass() {
  smoke_cell "$1" pass "$2" "" "${3:-}" "${4:-}"
}

smoke_skip() {
  smoke_cell "$1" skipped "$2" "" "${3:-}" "${4:-}"
}

smoke_blocked() {
  # J 类：取不到证据。不算通过也不算失败，且**不开 issue**。
  smoke_cell "$1" blocked "$2" "" "${3:-}" "${4:-}"
}

smoke_fail() {
  # $1=action $2=category(A–J) $3=assertion [$4=detail] [$5=upgrade_mode]
  smoke_cell "$1" fail "$3" "$2" "${4:-}" "${5:-}"
  SMOKE_FAILED=1
  export SMOKE_FAILED
}

smoke_finish() {
  smoke_require_python || return 1
  "${SMOKE_PYTHON}" "${SMOKE_COMMON_PY}" finish --file "${SMOKE_RESULT_FILE}"
}

# 运行一条「允许失败」的命令：把退出码与输出留下来，不中断脚本。
# 用法：if smoke_soft "安装 0.2.4" ./setup.exe /S; then ...
smoke_soft() {
  _so_label=$1
  shift
  smoke_log "运行：${_so_label}"
  if "$@"; then
    SMOKE_LAST_STATUS=0
  else
    SMOKE_LAST_STATUS=$?
  fi
  export SMOKE_LAST_STATUS
  smoke_log "${_so_label} → exit=${SMOKE_LAST_STATUS}"
  [ "${SMOKE_LAST_STATUS}" -eq 0 ]
}

# ---------------------------------------------------------------------------
# 资产 / 网络
# ---------------------------------------------------------------------------

smoke_sha256() {
  # 统一走 python：Windows runner 上 sha256sum 不一定有，shasum 在 macOS 上大小写不同
  smoke_require_python || return 1
  "${SMOKE_PYTHON}" "${SMOKE_COMMON_PY}" sha256 --file "$1"
}

smoke_http() {
  # $1=url [$2=期望状态码，默认 200]；输出 common.py 的 probe JSON（不抛错）
  smoke_require_python || return 1
  _sh_expect=${2:-200}
  "${SMOKE_PYTHON}" "${SMOKE_COMMON_PY}" probe --url "$1" --expect "${_sh_expect}"
}

# 从 JSON 里取一个值（jq 缺失时的兜底；driver 里能用 jq 就优先 jq）。
smoke_json_get() {
  # $1=json 文本 $2=点路径（如 apiVersion）
  smoke_require_python || return 1
  printf '%s' "$1" | "${SMOKE_PYTHON}" -c '
import json, sys
path = sys.argv[1].split(".")
data = json.load(sys.stdin)
for key in path:
    if isinstance(data, dict):
        data = data.get(key)
    else:
        data = None
    if data is None:
        break
print("" if data is None else data)
' "$2"
}

# ---------------------------------------------------------------------------
# 初始化自检
# ---------------------------------------------------------------------------

if [ -z "${SMOKE_PYTHON}" ]; then
  printf '::warning::缺少 python3：release-smoke 的结果契约无法写出\n' >&2
fi

if [ -n "${SMOKE_RESULT_FILE:-}" ]; then
  mkdir -p "$(dirname -- "${SMOKE_RESULT_FILE}")"
fi

mkdir -p "${SMOKE_RESULT_DIR}" "${SMOKE_ARTIFACT_DIR}"

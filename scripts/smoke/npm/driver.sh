#!/usr/bin/env bash
# npm CLI 形态的发版后验收驱动（设计 §5.4 的 N1–N6）。
#
# 验证的是 **npm 用户真实的升级路径**：`npm i -g workbuddy-shift@0.2.4` → `@0.2.5`
# （设计 §0.4 的 `npm-upgrade`），以及升级后 `serve` 能起来并答 `/api/status`。
#
# 硬约定（设计 §2.2 / §9.2）：
# - **恒 exit 0**：断言写进 `common.py cell`，判定交给 Gate；异常落 fail 格；
# - 只连**官方源**（`--registry=https://registry.npmjs.org`）：镜像源会掩盖「平台分包没发布」；
# - 全局安装装到临时 prefix（`npm i -g --prefix ...`），不污染 runner 的全局 node_modules；
# - `WB_SHIFT_HOME` 指向新的临时空目录。
#
# 用法：
#   bash scripts/smoke/npm/driver.sh --version 0.2.5 --prev 0.2.4
#   bash scripts/smoke/npm/driver.sh --version 0.2.5 --prev 0.2.4 --out /tmp/npm.json
set -eu

VERSION=""
PREV=""
OUT=""
REGISTRY="https://registry.npmjs.org"
PORT=""
PREFIX=""
PACKAGE="workbuddy-shift"

usage() {
  cat <<'USAGE'
用法：bash scripts/smoke/npm/driver.sh --version <当前版本> [--prev <上一版>] [--out <结果JSON>] [--registry <源>] [--port <端口>]

  --version   目标版本（0.2.5），必填
  --prev      上一版（0.2.4）；为空时升级格记 skipped
  --out       结果 JSON 路径（默认 <repo>/artifacts/release-smoke/npm.json）
  --registry  npm 源（默认 https://registry.npmjs.org；只认官方源，避免镜像掩盖缺包）
  --port      serve 端口（默认自动挑一个空闲端口）

本脚本恒 exit 0；判定由 Gate 步骤（render-conclusion.py --gate）负责。
USAGE
}

while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION=${2:-}; shift 2 ;;
    --prev | --prev-version) PREV=${2:-}; shift 2 ;;
    --out) OUT=${2:-}; shift 2 ;;
    --registry) REGISTRY=${2:-}; shift 2 ;;
    --port) PORT=${2:-}; shift 2 ;;
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

DRIVER_DIR=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH='' cd -- "${DRIVER_DIR}/../../.." && pwd)
case "${OUT}" in
  "") OUT="${REPO_ROOT}/artifacts/release-smoke/npm.json" ;;
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
# 证据落在结果 JSON 旁边（`artifacts/release-smoke/evidence/npm/`）：workflow 上传的是
# 仓库内的 `path: artifacts/`，放 RUNNER_TEMP 的日志不会被上传。
EVID=$(smoke_evidence_dir npm)
PREFIX="${WORK}/npm-prefix"
BIN=""
SERVE_PID=""

SMOKE_FINISHED=""
finish_and_exit() {
  if [ -n "${SMOKE_FINISHED}" ]; then
    return 0
  fi
  SMOKE_FINISHED=1
  if [ -n "${SERVE_PID}" ]; then
    kill -TERM "${SERVE_PID}" >/dev/null 2>&1 || true
    sleep 1
    kill -9 "${SERVE_PID}" >/dev/null 2>&1 || true
  fi
  smoke_finish || true
  smoke_log "npm driver 结束：结果 ${SMOKE_RESULT_FILE} · 证据 ${EVID}"
  exit 0
}
trap finish_and_exit EXIT

# 去掉尾随空白（含 CR）：Windows 上的 python / npm 输出是 CRLF，多留一个 CR 会让
# `[ "$x" = "0.2.5" ]` 永不成立——本地在 Windows 上自测时就是这样误报的。
strip_cr() {
  printf '%s' "${1:-}" | sed -e 's/[[:space:]]*$//'
}

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
' "${2}" | sed -e 's/[[:space:]]*$//'
}

# npm 全局安装的 bin 落点随平台不同：Linux/macOS 是 `<prefix>/bin/<pkg>`，
# Windows 是 `<prefix>/<pkg>.cmd`。不写死路径，装完再发现。
resolve_bin() {
  for _candidate in "${PREFIX}/bin/${PACKAGE}" "${PREFIX}/${PACKAGE}" "${PREFIX}/${PACKAGE}.cmd" "${PREFIX}/${PACKAGE}.exe"; do
    if [ -f "${_candidate}" ]; then
      printf '%s\n' "${_candidate}"
      return 0
    fi
  done
  return 1
}

npm_run() {
  # 统一带上官方源：镜像源的「同步延迟」会把「平台分包没发布」伪装成「网络抖动」。
  npm "$@" --registry "${REGISTRY}"
}

if [ -z "${PORT}" ]; then
  PORT=$("${SMOKE_PYTHON}" -c 'import socket
sock = socket.socket()
sock.bind(("127.0.0.1", 0))
print(sock.getsockname()[1])
sock.close()')
  PORT=$(strip_cr "${PORT}")
fi

# ---------------------------------------------------------------------------
# N0：建结果
# ---------------------------------------------------------------------------
smoke_emit npm "${VERSION}" "${PREV}" npm-upgrade

smoke_group "环境"
smoke_log "node=$(node --version 2>/dev/null || echo 无) npm=$(npm --version 2>/dev/null || echo 无)"
smoke_log "registry=${REGISTRY} version=${VERSION} prev=${PREV:-—} port=${PORT} prefix=${PREFIX}"
smoke_endgroup

# ---------------------------------------------------------------------------
# N1：平台分包齐全（缺包是发版侧问题 → B 类）
# ---------------------------------------------------------------------------
smoke_group "N1 平台分包"
PLATFORMS=$(npm_run view "${PACKAGE}@${VERSION}" optionalDependencies --json 2>"${EVID}/npm-view-optional.log" || true)
printf '%s\n' "${PLATFORMS}" >"${EVID}/optional-dependencies.json"
PLATFORMS_KEYS=$(printf '%s' "${PLATFORMS}" | "${SMOKE_PYTHON}" -c '
import json, sys
try:
    data = json.load(sys.stdin)
except Exception:
    raise SystemExit
if isinstance(data, dict):
    for name in data.keys():
        sys.stdout.write(str(name) + "\n")
' 2>/dev/null || true)

if [ -z "${PLATFORMS_KEYS}" ]; then
  smoke_fail install B "读到 ${PACKAGE}@${VERSION} 的 optionalDependencies" "npm view 返回空，见 npm-view-optional.log"
else
  _bad=""
  _count=0
  while IFS= read -r _pkg; do
    _pkg=$(strip_cr "${_pkg}")
    if [ -z "${_pkg}" ]; then
      continue
    fi
    _count=$((_count + 1))
    _ver=$(npm_run view "${_pkg}@${VERSION}" version 2>/dev/null || true)
    _ver=$(strip_cr "${_ver}")
    if [ "${_ver}" != "${VERSION}" ]; then
      _bad="${_bad} ${_pkg}=${_ver:-缺失}"
    fi
  done <<EOF
${PLATFORMS_KEYS}
EOF
  if [ -n "${_bad}" ]; then
    smoke_fail install B "平台分包均为 ${VERSION}" "不一致：${_bad}"
  else
    smoke_pass install "平台分包齐全（${_count} 个均为 ${VERSION}）"
  fi
fi
smoke_endgroup

# ---------------------------------------------------------------------------
# N2–N3：装上一版 → 断言版本
# ---------------------------------------------------------------------------
smoke_group "N2–N3 安装上一版"
if [ -z "${PREV}" ]; then
  smoke_skip install "npm i -g ${PACKAGE}@上一版" "没有上一版（首次发版）"
  smoke_skip version "上一版 version 输出含上一版号" "没有上一版"
else
  if npm_run install -g --prefix "${PREFIX}" "${PACKAGE}@${PREV}" >"${EVID}/npm-install-prev.log" 2>&1; then
    smoke_pass install "npm i -g ${PACKAGE}@${PREV}（prefix=${PREFIX}）"
    BIN=$(resolve_bin || true)
    if [ -z "${BIN}" ]; then
      smoke_fail install E "安装后存在可执行文件（${PREFIX} 下）" "npm 安装成功但 bin 缺失"
    else
      _out=$(strip_cr "$("${BIN}" version 2>&1 || true)")
      printf '%s\n' "${_out}" >"${EVID}/version-prev.txt"
      case "${_out}" in
        *"${PREV}"*) smoke_pass version "上一版 version 输出含 ${PREV}（${_out}）" ;;
        *) smoke_fail version H "上一版 version 输出含 ${PREV}" "实际输出：${_out:-空}" ;;
      esac
    fi
  else
    smoke_fail install B "npm i -g ${PACKAGE}@${PREV}" "见 npm-install-prev.log"
    smoke_skip version "上一版 version 输出" "安装失败，无从验证"
  fi
fi
smoke_endgroup

# ---------------------------------------------------------------------------
# N4–N5：升级到当前版本 → 断言版本（`npm-upgrade` 口径）
# ---------------------------------------------------------------------------
smoke_group "N4–N5 升级"
if [ -z "${PREV}" ]; then
  smoke_skip upgrade "npm i -g 升级（无上一版）" "首次发版，升级格不适用"
  if npm_run install -g --prefix "${PREFIX}" "${PACKAGE}@${VERSION}" >"${EVID}/npm-install-curr.log" 2>&1; then
    smoke_pass install "npm i -g ${PACKAGE}@${VERSION}（首次安装）"
  else
    smoke_fail install B "npm i -g ${PACKAGE}@${VERSION}" "见 npm-install-curr.log"
  fi
else
  if npm_run install -g --prefix "${PREFIX}" "${PACKAGE}@${VERSION}" >"${EVID}/npm-install-curr.log" 2>&1; then
    smoke_pass upgrade "npm i -g 从 ${PREV} 升级到 ${VERSION}"
  else
    smoke_fail upgrade B "npm i -g 从 ${PREV} 升级到 ${VERSION}" "见 npm-install-curr.log"
  fi
fi

BIN=$(resolve_bin || true)
if [ -n "${BIN}" ]; then
  _out=$(strip_cr "$("${BIN}" version 2>&1 || true)")
  printf '%s\n' "${_out}" >"${EVID}/version-curr.txt"
  case "${_out}" in
    *"${VERSION}"*)
      smoke_pass version "升级后 version 输出含 ${VERSION}（${_out}）"
      _npm_version=$(printf '%s' "${_out}" | sed -n 's/.*\([0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\).*/\1/p' | head -n 1)
      "${SMOKE_PYTHON}" "${SMOKE_COMMON_PY}" set --file "${SMOKE_RESULT_FILE}" \
        --set "npmVersion=${_npm_version:-${VERSION}}" --set "npxPackageVersion=${VERSION}"
      ;;
    *) smoke_fail version H "升级后 version 输出含 ${VERSION}" "实际输出：${_out:-空}" ;;
  esac
else
  smoke_fail version E "升级后存在可执行文件（${PREFIX} 下）" "npm 安装成功但 bin 缺失"
fi
smoke_endgroup

# ---------------------------------------------------------------------------
# N6：serve + HTTP（`/api/status` 与 `/`）
# ---------------------------------------------------------------------------
smoke_group "N6 serve + HTTP"
SANDBOX_SERVE=""
if [ -z "${BIN}" ]; then
  smoke_skip serve "serve 起来并应答 /api/status" "可执行文件缺失"
  smoke_skip api "GET /api/status 200 且 version 匹配" "可执行文件缺失"
else
  SANDBOX_SERVE=$(smoke_sandbox)
  # 前缀赋值而不是 `env`：Windows 上的 bin 是 .cmd，由 shell 负责拉起。
  WB_SHIFT_HOME="${SANDBOX_SERVE}" "${BIN}" serve --no-open --port "${PORT}" \
    >"${EVID}/serve.log" 2>&1 &
  SERVE_PID=$!
  smoke_log "serve pid=${SERVE_PID} port=${PORT} home=${SANDBOX_SERVE}"

  PROBE=""
  _deadline=$(( $(date +%s) + 60 ))
  while [ "$(date +%s)" -lt "${_deadline}" ]; do
    if ! kill -0 "${SERVE_PID}" 2>/dev/null; then
      break
    fi
    PROBE=$(smoke_http "http://127.0.0.1:${PORT}/api/status" 200 2>/dev/null || true)
    if [ -n "${PROBE}" ] && [ "$(json_get "${PROBE}" ok)" = "True" ]; then
      break
    fi
    sleep 2
  done

  if ! kill -0 "${SERVE_PID}" 2>/dev/null; then
    smoke_fail serve F "serve 进程在探测期间不退出" "已退出，见 serve.log"
    smoke_skip api "GET /api/status 200 且 version 匹配" "serve 已退出"
  else
    _status=$(json_get "${PROBE}" status)
    _body=$(json_get "${PROBE}" body)
    printf '%s\n' "${_body}" >"${EVID}/api-status.json"
    if [ "${_status}" != "200" ]; then
      smoke_fail api F "GET /api/status 返回 200" "实际 status=${_status:-无响应}，detail=$(json_get "${PROBE}" error)"
    else
      _api_version=$(printf '%s' "${_body}" | "${SMOKE_PYTHON}" -c '
import json, sys
try:
    data = json.load(sys.stdin)
except Exception:
    print("")
    raise SystemExit
print((data.get("version") or "") if isinstance(data, dict) else "")
' 2>/dev/null || true)
      _api_version=$(strip_cr "${_api_version}")
      if [ "${_api_version}" != "${VERSION}" ]; then
        smoke_fail api H "/api/status 的 version == ${VERSION}" "实际 ${_api_version:-读不到}"
      else
        smoke_pass api "GET /api/status 200 且 version == ${VERSION}"
        "${SMOKE_PYTHON}" "${SMOKE_COMMON_PY}" set --file "${SMOKE_RESULT_FILE}" --set "apiVersion=${_api_version}"
      fi
    fi

    # `/` 必须 200 且是 HTML（前端产物装进 npm 包后同样要能被 serve 出来）
    ROOT_PROBE=$(smoke_http "http://127.0.0.1:${PORT}/" 200 2>/dev/null || true)
    _root_status=$(json_get "${ROOT_PROBE}" status)
    _root_type=$(json_get "${ROOT_PROBE}" contentType)
    case "${_root_type}" in
      text/html*) _root_type_ok="yes" ;;
      *) _root_type_ok="no" ;;
    esac
    if [ "${_root_status}" = "200" ] && [ "${_root_type_ok}" = "yes" ]; then
      smoke_pass serve "GET / 200 且 Content-Type 是 text/html（${_root_type}）"
    else
      smoke_fail serve F "GET / 200 且 Content-Type: text/html" "status=${_root_status:-无响应} contentType=${_root_type:-无}"
    fi
  fi
fi
smoke_endgroup

# ---------------------------------------------------------------------------
# 清理：交给 EXIT trap（kill serve → finish → exit 0）
# ---------------------------------------------------------------------------
if [ -n "${SERVE_PID}" ]; then
  kill -TERM "${SERVE_PID}" >/dev/null 2>&1 || true
  sleep 1
  SERVE_PID=""
fi
exit 0

#!/usr/bin/env bash
# 容器（GHCR 镜像）形态的发版后验收驱动（设计 §5.5 的 K1–K7）。
#
# 容器形态没有「升级」，所以四件事一起验：
#   1. `:<version>` 与 `:latest` 指向**同一个 digest**（latest 没跟上版本是发版侧缺陷）；
#   2. `docker run` 起得来、90s 内 `healthy`（镜像自带 HEALTHCHECK 探 `/api/status`）；
#   3. `/api/status` 200 且 version == 目标版本；
#   4. 非 root（uid 1000）+ 数据卷可写并**宿主可读**。
#
# 硬约定（设计 §2.2 / §9.2）：
# - **恒 exit 0**：断言写进 `common.py cell`，判定交给 Gate；异常落 fail 格；
# - 失败时把 `docker logs` 落进 evidence 目录，再去 `docker rm -f`——日志删了就没法查；
# - 容器名带随机后缀，避免并发 run 互相踩。
#
# 用法：
#   bash scripts/smoke/docker/driver.sh --version 0.2.5
#   bash scripts/smoke/docker/driver.sh --version 0.2.5 --image ghcr.io/owner/repo
set -eu

VERSION=""
OUT=""
IMAGE="ghcr.io/magicapple123/workbuddy-shift"
PORT=""
CONTAINER_PORT=57910

usage() {
  cat <<'USAGE'
用法：bash scripts/smoke/docker/driver.sh --version <当前版本> [--out <结果JSON>] [--image <镜像>] [--port <宿主端口>]

  --version  目标版本（0.2.5），必填
  --out      结果 JSON 路径（默认 <repo>/artifacts/release-smoke/docker.json）
  --image    镜像名（默认 ghcr.io/magicapple123/workbuddy-shift，与 preflight.py 同源）
  --port     宿主端口（默认自动挑空闲端口，映射到容器 57910）

本脚本恒 exit 0；判定由 Gate 步骤（render-conclusion.py --gate）负责。
USAGE
}

while [ $# -gt 0 ]; do
  case "$1" in
    --version) VERSION=${2:-}; shift 2 ;;
    --out) OUT=${2:-}; shift 2 ;;
    --image) IMAGE=${2:-}; shift 2 ;;
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

DRIVER_DIR=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH='' cd -- "${DRIVER_DIR}/../../.." && pwd)
case "${OUT}" in
  "") OUT="${REPO_ROOT}/artifacts/release-smoke/docker.json" ;;
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
# 证据落在结果 JSON 旁边（`artifacts/release-smoke/evidence/docker/`）：workflow 上传的是
# 仓库内的 `path: artifacts/`，放 RUNNER_TEMP 的 container.log / volume-write.log 不会被上传
# （过去 detail 写着「见 volume-write.log」，artifact 里却只有 JSON）。
EVID=$(smoke_evidence_dir docker)
VOLUME_DIR="${WORK}/volume"
mkdir -p "${VOLUME_DIR}"
# 容器按设计以 uid 1000 运行（Dockerfile 的 `USER app`，uid/gid 都是 1000），而宿主目录由
# runner 的用户创建、默认 0755 → 容器里的 1000 既不是 owner 也不在 group，写不进去
# （run 44941972411 的 volume=I）。这是**驱动的准备步骤**缺失，不是产品缺陷：CI 的临时目录，
# 直接把权限放开。
chmod 0777 "${VOLUME_DIR}"
ls -ld "${VOLUME_DIR}" >"${EVID}/volume-dir.txt" 2>&1 || true
NAME="release-smoke-$$-$(date +%s)"
CREATED=""

SMOKE_FINISHED=""
finish_and_exit() {
  if [ -n "${SMOKE_FINISHED}" ]; then
    return 0
  fi
  SMOKE_FINISHED=1
  if [ -n "${CREATED}" ]; then
    docker logs "${NAME}" >"${EVID}/container.log" 2>&1 || true
    docker rm -f "${NAME}" >/dev/null 2>&1 || true
  fi
  smoke_finish || true
  smoke_log "docker driver 结束：结果 ${SMOKE_RESULT_FILE} · 证据 ${EVID}"
  exit 0
}
trap finish_and_exit EXIT

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

# 镜像的 RepoDigests → 只留 digest 部分，用于比较「:<ver> 与 :latest 是否同一份」
image_digests() {
  docker image inspect --format '{{range .RepoDigests}}{{println .}}{{end}}' "${1}" 2>/dev/null |
    sed -e 's/.*@//' -e '/^$/d' | sort -u | tr '\n' ' '
}

if [ -z "${PORT}" ]; then
  PORT=$("${SMOKE_PYTHON}" -c 'import socket
sock = socket.socket()
sock.bind(("127.0.0.1", 0))
print(sock.getsockname()[1])
sock.close()')
fi

VERSIONED="${IMAGE}:${VERSION}"
LATEST="${IMAGE}:latest"

# ---------------------------------------------------------------------------
# K0：建结果（容器形态没有 upgradeMode：null）
# ---------------------------------------------------------------------------
smoke_emit docker "${VERSION}"

smoke_group "环境"
smoke_log "docker=$(docker --version 2>/dev/null || echo 无) image=${IMAGE} port=${PORT}"
smoke_log "version=${VERSION} containerPort=${CONTAINER_PORT} name=${NAME}"
if ! command -v docker >/dev/null 2>&1; then
  smoke_fail pull I "docker 可用" "runner 上没有 docker CLI"
fi
smoke_endgroup

# ---------------------------------------------------------------------------
# K1：pull `:<version>` 与 `:latest`，digest 必须相同
# ---------------------------------------------------------------------------
smoke_group "K1 docker pull"
PULL_VERSIONED_OK="no"
PULL_LATEST_OK="no"
if docker pull "${VERSIONED}" >"${EVID}/pull-versioned.log" 2>&1; then
  PULL_VERSIONED_OK="yes"
  smoke_pass pull "docker pull ${VERSIONED}"
else
  smoke_fail pull C "docker pull ${VERSIONED}" "manifest 缺失或网络失败，见 pull-versioned.log"
fi
if docker pull "${LATEST}" >"${EVID}/pull-latest.log" 2>&1; then
  PULL_LATEST_OK="yes"
  smoke_pass pull "docker pull ${LATEST}"
else
  smoke_fail pull C "docker pull ${LATEST}" "manifest 缺失或网络失败，见 pull-latest.log"
fi

if [ "${PULL_VERSIONED_OK}" = "yes" ] && [ "${PULL_LATEST_OK}" = "yes" ]; then
  DIGEST_VERSIONED=$(image_digests "${VERSIONED}")
  DIGEST_LATEST=$(image_digests "${LATEST}")
  {
    printf 'versioned=%s\nlatest=%s\n' "${DIGEST_VERSIONED}" "${DIGEST_LATEST}"
  } >"${EVID}/digests.txt"
  if [ -n "${DIGEST_VERSIONED}" ] && [ "${DIGEST_VERSIONED}" = "${DIGEST_LATEST}" ]; then
    smoke_pass pull ":${VERSION} 与 :latest digest 相同（${DIGEST_VERSIONED}）"
  else
    smoke_fail pull C ":${VERSION} 与 :latest 指向同一 digest" \
      "versioned='${DIGEST_VERSIONED}' latest='${DIGEST_LATEST}'"
  fi
else
  smoke_fail pull C ":${VERSION} 与 :latest 指向同一 digest" "镜像没拉全，无法比较 digest"
fi
smoke_endgroup

# ---------------------------------------------------------------------------
# K2：docker run（端口映射 + 数据卷）
# ---------------------------------------------------------------------------
smoke_group "K2 docker run"
RUN_ID=""
# K2 只负责「把容器拉起来」，不自己写 health/api/nonroot/volume 的格子——
# 那四格由 K3–K6 统一处理（容器没起来时它们记 skipped），否则同一个动作会出现两格。
# 但「容器没起来」必须有且只有一个 fail：写在 K3 的 health 格里（否则整份结果全是 skipped，
# Gate 会误判为「都跳过 = 通过」）。
RUN_ERROR=""
if [ "${PULL_VERSIONED_OK}" != "yes" ]; then
  RUN_ERROR="镜像未拉到（docker pull ${VERSIONED} 失败），无法运行"
elif RUN_ID=$(docker run -d --name "${NAME}" \
  -p "127.0.0.1:${PORT}:${CONTAINER_PORT}" \
  -v "${VOLUME_DIR}:/data/vol" \
  "${VERSIONED}" 2>"${EVID}/run.err"); then
  CREATED="yes"
  smoke_log "容器已启动 id=${RUN_ID} name=${NAME} port=${PORT} volume=${VOLUME_DIR}"
else
  CREATED=""
  RUN_ERROR="docker run 失败：$(tr -d '\n' <"${EVID}/run.err" 2>/dev/null | head -c 300)"
fi
if [ -z "${CREATED}" ]; then
  smoke_log "容器未起来：${RUN_ERROR}"
fi
smoke_endgroup

# ---------------------------------------------------------------------------
# K3：health（90s 内 healthy）
# ---------------------------------------------------------------------------
smoke_group "K3 health"
if [ -z "${CREATED}" ]; then
  # 唯一一个「容器没起来」的 fail（api/nonroot/volume 记 skipped，避免整份结果被误判通过）。
  smoke_fail health I "容器 90s 内 healthy" "${RUN_ERROR:-容器未创建}"
else
  _health=""
  _deadline=$(( $(date +%s) + 90 ))
  while [ "$(date +%s)" -lt "${_deadline}" ]; do
    _health=$(docker inspect --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}none{{end}}' "${NAME}" 2>/dev/null || true)
    case "${_health}" in
      healthy) break ;;
      unhealthy) break ;;
    esac
    if ! docker inspect --format '{{.State.Running}}' "${NAME}" 2>/dev/null | grep -q true; then
      _health="exited"
      break
    fi
    sleep 3
  done
  printf 'health=%s\n' "${_health}" >"${EVID}/health.txt"
  if [ "${_health}" = "healthy" ]; then
    smoke_pass health "容器 90s 内 healthy"
  else
    docker logs "${NAME}" >"${EVID}/container.log" 2>&1 || true
    smoke_fail health I "容器 90s 内 healthy" "实际 health=${_health:-超时未判定}，见 container.log"
  fi
fi
smoke_endgroup

# ---------------------------------------------------------------------------
# K4：/api/status
# ---------------------------------------------------------------------------
smoke_group "K4 /api/status"
if [ -z "${CREATED}" ]; then
  smoke_skip api "GET /api/status 200 且 version 匹配" "容器未运行"
else
  PROBE=""
  _deadline=$(( $(date +%s) + 60 ))
  while [ "$(date +%s)" -lt "${_deadline}" ]; do
    PROBE=$(smoke_http "http://127.0.0.1:${PORT}/api/status" 200 2>/dev/null || true)
    if [ -n "${PROBE}" ] && [ "$(json_get "${PROBE}" ok)" = "True" ]; then
      break
    fi
    sleep 3
  done
  _status=$(json_get "${PROBE}" status)
  _body=$(json_get "${PROBE}" body)
  printf '%s\n' "${_body}" >"${EVID}/api-status.json"
  if [ "${_status}" != "200" ]; then
    smoke_fail api I "GET /api/status 返回 200" "实际 status=${_status:-无响应} error=$(json_get "${PROBE}" error)"
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
    if [ "${_api_version}" != "${VERSION}" ]; then
      smoke_fail api H "/api/status 的 version == ${VERSION}" "实际 ${_api_version:-读不到}"
    else
      smoke_pass api "GET /api/status 200 且 version == ${VERSION}"
      "${SMOKE_PYTHON}" "${SMOKE_COMMON_PY}" set --file "${SMOKE_RESULT_FILE}" --set "apiVersion=${_api_version}"
    fi
  fi
fi
smoke_endgroup

# ---------------------------------------------------------------------------
# K5：非 root（uid 1000）
# ---------------------------------------------------------------------------
smoke_group "K5 非 root"
if [ -z "${CREATED}" ]; then
  smoke_skip nonroot "容器内 uid == 1000" "容器未运行"
else
  UID_ACTUAL=$(docker exec "${NAME}" id -u 2>"${EVID}/uid.err" | tr -d '\r\n' || true)
  if [ "${UID_ACTUAL}" = "1000" ]; then
    smoke_pass nonroot "容器内 id -u == 1000"
  else
    smoke_fail nonroot I "容器内 id -u == 1000" "实际 ${UID_ACTUAL:-读不到}，见 uid.err"
  fi
fi
smoke_endgroup

# ---------------------------------------------------------------------------
# K6：数据卷（容器写、宿主读）
# ---------------------------------------------------------------------------
smoke_group "K6 数据卷"
if [ -z "${CREATED}" ]; then
  smoke_skip volume "容器写 /data/vol 后宿主可读" "容器未运行"
else
  _wlog="${EVID}/volume-write.log"
  if docker exec "${NAME}" sh -c 'echo ok > /data/vol/probe.txt' >"${_wlog}" 2>&1; then
    if [ -f "${VOLUME_DIR}/probe.txt" ] && [ "$(cat "${VOLUME_DIR}/probe.txt" 2>/dev/null | tr -d '\r\n')" = "ok" ]; then
      smoke_pass volume "容器写 /data/vol/probe.txt 且宿主可读（内容 ok）"
    else
      smoke_fail volume I "容器写的数据宿主可读" "宿主 ${VOLUME_DIR}/probe.txt 不存在或内容不符，见 evidence/docker/volume-write.log"
    fi
  else
    # 失败必须能自证「是谁没权限」：容器内 uid/gid、挂载点权限、宿主目录权限、touch 的 stderr
    # 一起落到同一份 evidence（过去 detail 只说「见 volume-write.log」，而那个文件没有上传）。
    {
      printf '\n--- 现场（写入失败时补采） ---\n'
      printf '## docker exec id\n'
      docker exec "${NAME}" id 2>&1 || true
      printf '## docker exec ls -ld /data/vol\n'
      docker exec "${NAME}" ls -ld /data/vol 2>&1 || true
      printf '## 宿主卷目录\n'
      ls -ld "${VOLUME_DIR}" 2>&1 || true
    } >>"${_wlog}" 2>&1
    _uid_line=$(docker exec "${NAME}" id 2>/dev/null | tr -d '\r\n' || true)
    _ls_line=$(docker exec "${NAME}" ls -ld /data/vol 2>/dev/null | tr -d '\r\n' || true)
    smoke_fail volume I "容器内写 /data/vol/probe.txt" \
      "id=${_uid_line:-读不到} · /data/vol=${_ls_line:-读不到} · touch 的 stderr 与宿主权限见 evidence/docker/volume-write.log"
  fi
fi
smoke_endgroup

# ---------------------------------------------------------------------------
# K7：清理（日志先落盘，再删容器）
# ---------------------------------------------------------------------------
smoke_group "K7 清理"
if [ -n "${CREATED}" ]; then
  docker logs "${NAME}" >"${EVID}/container.log" 2>&1 || true
  if docker rm -f "${NAME}" >"${EVID}/rm.log" 2>&1; then
    smoke_log "容器已删除：${NAME}"
  else
    smoke_warn "删除容器失败：$(cat "${EVID}/rm.log" 2>/dev/null)"
  fi
  CREATED=""
fi
smoke_endgroup

exit 0

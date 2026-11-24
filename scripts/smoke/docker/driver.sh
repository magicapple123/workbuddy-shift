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
# 镜像由独立的 `docker.yml` 用同一个 tag 触发构建，和本验收是**并行**的两条流水线：
# 正式版刚发布时镜像常常还在推。多架构 buildx 首次构建实测接近 1 小时（之后走 GHA 层缓存），
# 因此「短重试」盖不住这个窗口——先等工作流收敛再拉，才不会把「还差二十分钟」记成 C 类
# 并白烧一次处置。查不到工作流时（手动验收老 tag / 本地跑 / 无 gh）直接去拉，语义不变。
TAG="v${VERSION}"

# --- 预算（单位 s） -------------------------------------------------------
# job 的 timeout-minutes 是 55 min = 3300s，必须留够（issue #14 之后重算）：
#   「等 docker.yml 收敛 + 拉取」≤ TOTAL_WAIT_BUDGET_SECONDS（默认 2880s = 48 min）
#   + K2–K7（health 上限 90s + api 上限 60s + exec/清理 ~30s，按 240s 预留）
#   ≈ 3120s = 52 min，仍在 55 min 之内。
# 实测最坏还要再加 ≤60s：等待循环最后一个 sleep 最多超出上限 20s，拉取的最后一次尝试有
# 60s 的「单次限时」下限 → 2880 + 60 + 240 ≈ 53 min，仍有余量。
# job 被超时**掐断**比红更糟：结果 JSON 不完整，triage 只会看到「形态缺失」，
# 而 driver 的 trap 也来不及把证据落盘。所有等待都以总预算为硬上限。
TOTAL_WAIT_BUDGET_SECONDS="${SMOKE_DOCKER_TOTAL_WAIT_BUDGET_SECONDS:-2880}"
WAIT_DEADLINE=$((SECONDS + TOTAL_WAIT_BUDGET_SECONDS))
# 单步上限：等构建 45 min、拉取 20 min，两者再被上面的总预算各截一次。
BUILD_WAIT_SECONDS="${SMOKE_DOCKER_BUILD_WAIT_SECONDS:-2700}"
PULL_BUDGET_SECONDS="${SMOKE_DOCKER_PULL_BUDGET_SECONDS:-1200}"
# 单次 `docker pull` 的限时：没有它，一次网络黑洞（SYN 黑洞 / TLS 卡住）就能让
# 「20 分钟预算」变成一句空话，还可能把 job 拖过 55 min。
PULL_ATTEMPT_TIMEOUT_SECONDS="${SMOKE_DOCKER_PULL_ATTEMPT_TIMEOUT_SECONDS:-300}"
# 单次 `gh api`（查 docker.yml 的 run 状态）的限时：一次正常往返 <1s，但网络半挂
# （TCP 建连成功、之后无响应）时单次调用能拖到几十秒甚至更久，吃掉的正是「等构建」的
# 那 2700s 预算——它会让预算算术失真（detail 里写「等了 N 秒」实际含了不可控的空转）。
# 超时按「这次查不到」处理：调用方本来就把空串当 `no-workflow`（直接去拉），语义不变。
BUILD_STATE_API_TIMEOUT_SECONDS="${SMOKE_DOCKER_BUILD_STATE_API_TIMEOUT_SECONDS:-20}"
# 重试口径：次数上限 ∩ 预算上限；两次尝试之间 15s 起、逐次翻倍、封顶 60s。
# 15+30+60+60+60… 累加到 1200s 约 21 次，与默认次数上限 24 基本贴合
# （即「先撞预算」的那一档，撞上后日志里会写明是哪一条先到）。
#
# 与旧版的语义差异（写清是因为这些变量可能被 workflow / 人工复验覆盖）：
#   - SMOKE_DOCKER_PULL_INTERVAL_SECONDS：旧版是**固定**间隔（默认 20），
#     现在是**初始**间隔（默认 15）；
#   - 新增 SMOKE_DOCKER_PULL_MAX_INTERVAL_SECONDS（封顶，默认 60）、
#     SMOKE_DOCKER_PULL_BUDGET_SECONDS（预算，默认 1200）、
#     SMOKE_DOCKER_PULL_ATTEMPT_TIMEOUT_SECONDS（单次限时，默认 300）、
#     SMOKE_DOCKER_BUILD_STATE_API_TIMEOUT_SECONDS（查构建状态的单次 api 限时，默认 20）；
#   - SMOKE_DOCKER_PULL_ATTEMPTS 仍是「总次数上限」，默认 18 → 24。
# 为什么要盖住十几分钟：`manifest 可见 ≠ 立刻能拉`。仓库匿名拉 ghcr.io 时会命中
# 令牌/传播缓存与限流窗口，v0.3.2 见过 inspect 与 pull 结论不一致的现场；
# 旧的 18 × 20s ≈ 340s 太短。注意：v0.3.3（run 45381735149）的红**不是**这个原因，
# 那次根本没发起拉取，见 wait_for_docker_build 上方的注释。
PULL_ATTEMPTS="${SMOKE_DOCKER_PULL_ATTEMPTS:-24}"
PULL_INTERVAL_SECONDS="${SMOKE_DOCKER_PULL_INTERVAL_SECONDS:-15}"
PULL_MAX_INTERVAL_SECONDS="${SMOKE_DOCKER_PULL_MAX_INTERVAL_SECONDS:-60}"

smoke_log "K1 预算：等构建 ${BUILD_WAIT_SECONDS}s · 拉取 ${PULL_BUDGET_SECONDS}s · 总等待 ${TOTAL_WAIT_BUDGET_SECONDS}s（job timeout 55 min）"
# 登录态只影响「是否更容易被限流/拒绝」，不影响流程：CI 的 container-docker job 会先
# `docker login ghcr.io`（packages: read），本地与无凭据场景继续匿名拉取，两者都必须能跑。
if [ -f "${DOCKER_CONFIG:-${HOME:-/nonexistent}/.docker}/config.json" ] &&
  grep -qs 'ghcr\.io' "${DOCKER_CONFIG:-${HOME:-/nonexistent}/.docker}/config.json"; then
  smoke_log "ghcr 登录态：有（带凭据拉取）"
else
  smoke_log "ghcr 登录态：无（匿名拉取，本地 / 无凭据场景）"
fi

# 该 tag 上最近一次 push 触发的 `docker.yml` 结论，形如 `in_progress/-`、`completed/success`。
# 查不到（无仓库上下文 / 无 gh / 无对应 run / 本次 api 超时）时输出空串，调用方据此退回短重试。
docker_build_state() {
  [ -n "${GITHUB_REPOSITORY:-}" ] || return 0
  command -v gh >/dev/null 2>&1 || return 0
  # `// empty` 是必须的：没有对应 run 时 `.workflow_runs[0]` 是 null，直接取值会渲染成
  # `null/-`，让上游误以为「有个处于未知状态的构建」而一直等到上限。
  #
  # 单次 api 必须带显式限时（`smoke_timeout`，与 `docker pull` 用法一致）：**不让一次 api
  # 卡住吃掉等待预算**。等待循环每 20s 查一次，网络半挂时一次调用就能拖几十秒，等满 2700s
  # 的时间里这些空转会被算进「等了 N 秒」，`timeout` detail 里的数字也随之失真。
  # `|| _raw=""` 是**必须**的：`set -eu` 下超时返回 124（`timeout` 与 `smoke_timeout` 的兜底
  # 实现都返回 124），没有兜底会直接把 driver 打断（整份结果 JSON 不完整，比红更糟）。
  _raw=$(smoke_timeout "${BUILD_STATE_API_TIMEOUT_SECONDS}" gh api \
    "repos/${GITHUB_REPOSITORY}/actions/workflows/docker.yml/runs?event=push&branch=${TAG}&per_page=1" \
    --jq '.workflow_runs[0] // empty | "\(.status)/\(.conclusion // "-")"' 2>/dev/null) || _raw=""
  case "${_raw}" in
    ""|null/*|/*) return 0 ;;
    *) printf '%s' "${_raw}" ;;
  esac
}

# 等镜像工作流收敛：`ok`（可拉）/ `no-workflow`（查不到，直接去拉）/
# `build-<conclusion>`（构建真的失败）/ `timeout`（等到上限还没完）。
#
# ⚠️ 结果写进**全局变量** WAIT_RESULT，诊断日志一律走 stderr，**不要**改回
# `WAIT_RESULT=$(wait_for_docker_build)`：旧版就是这么踩的，也是 issue #14 的真因——
#   1. 命令替换会捕获函数内 `smoke_log` 的等待日志，WAIT_RESULT 变成
#      「[smoke] 等待 docker 镜像构建（in_progress/-…）\nok」这样的多行串；
#   2. 调用方的 `case` 只有 `ok|no-workflow` / `timeout` / `build-*` 三个分支且**没有兜底**，
#      多行串一个都不匹配 → 整段拉取被**静默跳过**，PULL_*_OK 保持 no；
#   3. 于是结果只剩一格「:ver 与 :latest 指向同一 digest：镜像没拉全」，`docker pull` 从未执行、
#      `pull-*.log` 从未创建（所以 artifact 里也没有），而那条等待日志被吞进变量，
#      job 日志里只剩下 224 秒空白 —— 现场（run 45381735149）看起来像「pull 重试了 224s 全失败」。
# 现在：返回值走全局变量 + 日志走 stderr + 调用方补兜底分支（三层都不能少）。
WAIT_RESULT=""
WAIT_LIMIT_SECONDS=0
wait_for_docker_build() {
  WAIT_RESULT=""
  WAIT_LIMIT_SECONDS=0
  if [ "${SMOKE_DOCKER_WAIT_BUILD:-1}" != "1" ]; then
    WAIT_RESULT="no-workflow"
    return 0
  fi
  _start=${SECONDS}
  _last_log=${SECONDS}
  # 本步上限 = min(等构建上限, 总预算剩余)，早就算好放进 WAIT_LIMIT_SECONDS 供 detail 引用。
  _limit=${BUILD_WAIT_SECONDS}
  _left=$((WAIT_DEADLINE - SECONDS))
  if [ "${_left}" -lt "${_limit}" ]; then
    _limit=${_left}
  fi
  if [ "${_limit}" -lt 0 ]; then
    _limit=0
  fi
  WAIT_LIMIT_SECONDS=${_limit}
  while :; do
    _state=$(docker_build_state)
    case "${_state}" in
      "") WAIT_RESULT="no-workflow"; return 0 ;;
      completed/success) WAIT_RESULT="ok"; return 0 ;;
      completed/*) WAIT_RESULT="build-${_state#completed/}"; return 0 ;;
    esac
    if [ $((SECONDS - _start)) -ge "${_limit}" ]; then
      WAIT_RESULT="timeout"
      return 0
    fi
    if [ $((SECONDS - _last_log)) -ge 60 ]; then
      smoke_log "等待 docker 镜像构建（${_state}，已等 $((SECONDS - _start))s，上限 ${_limit}s）" >&2
      _last_log=${SECONDS}
    fi
    # 最多超出上限 20s（最后一个 sleep），已计入总预算裕量。
    sleep 20
  done
}

# 末次尝试**真实**的单次限时 → 全局 PULL_LAST_ATTEMPT_SECS（纯数字秒）/ PULL_LAST_ATTEMPT_EXACT。
#
# 为什么不能直接拿 `PULL_ATTEMPT_TIMEOUT_SECONDS` 去写 detail：`pull_with_retry` 里单次限时是
#   min(PULL_ATTEMPT_TIMEOUT_SECONDS, max(本次剩余预算, 60))
# 预算将尽时末次只跑 60–299s。若 detail 固定打印 300s，读者会以为「跑满了 5 分钟才被杀」，
# 而现场可能 1 分钟就被杀了——「网络慢」和「预算先到」的处置方向完全相反，日志不能自己说谎。
#
# 秒数来源：`pull_with_retry` 每次失败都会写一条记账行
#   `[smoke] pull <img> 失败（第 n/m 次，exit=<rc>，单次限时 <secs>s，累计用时 <t>s）`
# 这里按同一格式解析**末次**带 `exit=` 的那条（只取 `单次限时 <secs>s`）；日志缺失 / 被截断 /
# 停在「N s 后重试」这类行上时解析不到，回落到默认上限，调用方据此改用「≤」措辞。
#
# 走全局变量而不是 `$( )`：调用方（`pull_fail_detail`）要同时用「秒数」和「措辞」，命令替换
# 会把赋值丢在子 shell 里，两处各解析一遍日志又容易漂移。
PULL_LAST_ATTEMPT_SECS="${PULL_ATTEMPT_TIMEOUT_SECONDS}"
PULL_LAST_ATTEMPT_EXACT="no"
pull_last_attempt_timeout() { # $1=日志路径 → 写全局，无返回值
  PULL_LAST_ATTEMPT_SECS="${PULL_ATTEMPT_TIMEOUT_SECONDS}"
  PULL_LAST_ATTEMPT_EXACT="no"
  if [ ! -f "$1" ]; then
    return 0
  fi
  # `grep 'exit='` 先把记账行挑出来：口径行（`[smoke] 口径：…单次限时 300s`）也含「单次限时」，
  # 不筛就会在「只有口径行、还没跑过任何一次」时把默认上限当成实测值。
  _plat=$(tr -d '\r' <"$1" 2>/dev/null |
    grep 'exit=' | tail -n 1 | grep -o '单次限时 [0-9][0-9]*s' | grep -o '[0-9][0-9]*' || true)
  case "${_plat}" in
    "" | *[!0-9]*) ;;
    *)
      PULL_LAST_ATTEMPT_SECS="${_plat}"
      PULL_LAST_ATTEMPT_EXACT="yes"
      ;;
  esac
}

# 末次尝试是不是被**我们**的限时杀掉（`smoke_timeout` 在超时时返回 124，与 `timeout` 一致，
# 也会被 `pull_with_retry` 写进日志行的 `exit=`）。命中时输出一句人话，空则无输出。
#
# 为什么需要它：`docker pull` 的进度条走 **stderr 且不带换行**，进程被 `timeout` 杀掉时
# 缓冲区里没写完的那一段**常常整条丢失** → 日志里只剩我们自己写的 `[smoke]` 记账行。
# 此时 detail 的「最后输出」只能回落到记账行（或干脆空），读起来像「docker 什么都没说」，
# 而真实原因是「我们的限时把它杀了」——两者处置方向完全不同，必须自己写出来。
# 判定只看**末次**的 `exit=`（日志里可能既有超时也有别的失败，读者关心的是最后那一次）。
#
# 秒数用**末次真实限时**（见 `pull_last_attempt_timeout`），不再写死默认 300s：
# 解析得到实测值写「≈Ns」，回落到默认上限时写「≤Ns」——两者都不承诺「跑满了这个数」。
pull_timeout_note() { # $1=日志路径 → 单行说明（末次尝试非超时则输出空）
  if [ ! -f "$1" ]; then
    return 0
  fi
  _ptn_last_exit=$(tr -d '\r' <"$1" 2>/dev/null | grep -o 'exit=[0-9]*' | tail -n 1 || true)
  case "${_ptn_last_exit}" in
    exit=124)
      # 直接在**当前 shell** 里取值（不要 `$(pull_last_attempt_timeout …)`），否则拿不到全局。
      pull_last_attempt_timeout "$1"
      if [ "${PULL_LAST_ATTEMPT_EXACT}" = "yes" ]; then
        _ptn_limit_text="≈${PULL_LAST_ATTEMPT_SECS}s"
      else
        _ptn_limit_text="≤${PULL_LAST_ATTEMPT_SECS}s"
      fi
      printf '单次尝试超时（%s 限时被杀；docker 的进度行不带换行，被杀时最后一段常未落盘）' \
        "${_ptn_limit_text}"
      ;;
  esac
}

# docker 自己报的最后一句话：拼进 fail 的 detail，让 issue 正文与结论矩阵直接看出是哪一类，
# 不必先去下载 artifact（issue #14 的正文只有一句「docker pull … 失败」）。
pull_fail_hint() { # $1=日志路径 [$2=末次尝试被限时杀掉的秒数] → 单行、截尾 240 字符的原始报错
  if [ ! -f "$1" ] || [ ! -s "$1" ]; then
    if [ -n "${2:-}" ]; then
      printf '（docker 未输出：单次尝试在 %ss 被限时杀掉，进度走 stderr 且无换行，未落盘）' "$2"
      return 0
    fi
    printf '（%s 缺失或为空：拉取没有任何输出）' "$(basename "$1")"
    return 0
  fi
  # 只看 **docker 自己的输出**（过滤掉我们写的 `[smoke]` 记账行），取最后一句：
  # detail 要回答的是「docker 说什么」，不是「我们重试了几次」。
  _pfh=$(tr -d '\r' <"$1" 2>/dev/null | grep -v '^\[smoke\]' | grep -v '^[[:space:]]*$' | tail -n 1 || true)
  if [ -z "${_pfh}" ]; then
    if [ -n "${2:-}" ]; then
      # 一个字都没吐且是**我们**杀的：不要拿 `[smoke]` 记账行冒充 docker 的原话（会被读成
      # 「docker 说了这句」），直接说明是被限时杀掉、输出未落盘。
      _pfh="（docker 未输出：单次尝试在 ${2}s 被限时杀掉，进度走 stderr 且无换行，未落盘）"
    else
      # docker 一个字都没吐（被限时杀掉 / 信号打断）：退回去看最后一行，至少证明「跑过」。
      _pfh=$(tr -d '\r' <"$1" 2>/dev/null | grep -v '^[[:space:]]*$' | tail -n 1 || true)
    fi
  fi
  _pfh=$(printf '%s' "${_pfh}" | tr '\n' ' ' | tr -s ' ' ' ')
  # 截尾按**字符**做：按字节截会把中文/UTF-8 切成半个字符，detail 里出现乱码比不截更糟。
  if [ -n "${SMOKE_PYTHON}" ]; then
    _pfh=$(
      printf '%s' "${_pfh}" | "${SMOKE_PYTHON}" -c 'import sys
text = sys.stdin.read().strip()
print(text[-240:] if len(text) > 240 else text)' 2>/dev/null || printf '%s' "${_pfh}"
    )
  fi
  printf '%s' "${_pfh:-（日志无可读内容）}"
}

# 一句话归因。**只用于 detail 文案**，不参与 A–I 分类（分类口径由 classify.py 定义）。
pull_fail_class() { # $1=日志路径
  if [ ! -f "$1" ]; then
    printf 'unknown（日志文件不存在，驱动缺陷）'
    return 0
  fi
  _pfc=$(tr -d '\r' <"$1" 2>/dev/null || true)
  case "${_pfc}" in
    *denied* | *unauthorized* | *"authentication required"* | *"incorrect username or password"*)
      printf 'auth（凭据/令牌被拒，容器仓库鉴权）'
      ;;
    *"manifest unknown"* | *"no such manifest"* | *"not found"*)
      printf 'manifest（该 tag 的 manifest 不存在）'
      ;;
    *toomanyrequests* | *"rate limit"* | *"429 Too Many"*)
      printf 'rate-limit（仓库拉取限流）'
      ;;
    *"TLS handshake timeout"* | *"connection reset"* | *"i/o timeout"* | *"no such host"* | *EOF* | *"temporary failure"* | *"dial tcp"*)
      printf 'network（DNS/TLS/连接层）'
      ;;
    "") printf 'unknown（拉取无任何输出，可能是 CLI 层被信号打断）' ;;
    *) printf 'unknown（见日志最后几行原文）' ;;
  esac
}

# 失败 detail：自证「重试了什么、撞的是次数还是预算、docker 说什么、完整日志在哪」。
# 末次尝试被单次限时杀掉时额外插一句「单次尝试超时（≈Ns）」——N 是**末次真实限时**
# （预算将尽时会被缩到 60s 下限），不是默认 300s；否则读者会把「docker 无输出」当成
# docker 自己的沉默，而真正的原因是我们把它杀了（两者的处置方向完全不同）。
pull_fail_detail() { # $1=日志路径
  # 先在本 shell 里取末次限时（全局），`pull_fail_hint` 也复用同一个数字，两处口径不会打架。
  pull_last_attempt_timeout "$1"
  _pfd_timeout=$(pull_timeout_note "$1")
  _pfd_timeout_part=""
  _pfd_timeout_secs=""
  if [ -n "${_pfd_timeout}" ]; then
    _pfd_timeout_part="${_pfd_timeout} · "
    _pfd_timeout_secs="${PULL_LAST_ATTEMPT_SECS}"
  fi
  printf '重试上限 %s 次 / 预算 %ss 用尽 · 归因 %s · %s最后输出：%s · 完整日志 evidence/docker/%s（随 smoke-docker artifact 上传）' \
    "${PULL_ATTEMPTS}" "${PULL_BUDGET_SECONDS}" \
    "$(pull_fail_class "$1")" "${_pfd_timeout_part}" \
    "$(pull_fail_hint "$1" "${_pfd_timeout_secs}")" "$(basename "$1")"
}

# 拉取：次数上限 ∩ 时间预算，退避间隔 15s 起、翻倍、封顶 60s。
# 日志**追加**写入 evidence 下的文件（每次尝试的 docker 原文都在），并在开头写一行口径，
# 保证「文件一定存在且不空」——证据文件缺失时，结论里的「见 xxx.log」就是一句假话。
pull_with_retry() { # $1=镜像 $2=日志路径；成功 return 0，次数或预算用尽 return 1
  _pull_start=${SECONDS}
  # 本次拉取的截止时间 = min(本地预算, 总等待预算)，后者的裕量留给 K2–K7。
  _pull_deadline=$((SECONDS + PULL_BUDGET_SECONDS))
  if [ "${_pull_deadline}" -gt "${WAIT_DEADLINE}" ]; then
    _pull_deadline=${WAIT_DEADLINE}
  fi
  _attempt=1
  _interval=${PULL_INTERVAL_SECONDS}
  {
    printf '[smoke] pull %s\n' "$1"
    printf '[smoke] 口径：次数上限 %s · 初始间隔 %ss（×2 封顶 %ss）· 预算 %ss · 单次限时 %ss\n' \
      "${PULL_ATTEMPTS}" "${PULL_INTERVAL_SECONDS}" "${PULL_MAX_INTERVAL_SECONDS}" \
      "${PULL_BUDGET_SECONDS}" "${PULL_ATTEMPT_TIMEOUT_SECONDS}"
  } >"$2"
  while [ "${_attempt}" -le "${PULL_ATTEMPTS}" ]; do
    # 单次限时取「本次尝试剩余预算」（最少 60s）：否则一次网络黑洞能把预算与 job 超时都击穿。
    _attempt_timeout=${PULL_ATTEMPT_TIMEOUT_SECONDS}
    _left=$((_pull_deadline - SECONDS))
    if [ "${_left}" -lt 60 ]; then
      _left=60
    fi
    if [ "${_left}" -lt "${_attempt_timeout}" ]; then
      _attempt_timeout=${_left}
    fi
    # 注意：`$?` 必须在 if 之前/之内取。`if cmd; then …; fi` 之后 `$?` 是**复合命令**的退出码
    # （无 else 分支时恒为 0），用它写 detail 会把「拉取失败」记成 exit=0。
    _rc=0
    smoke_timeout "${_attempt_timeout}" docker pull "$1" >>"$2" 2>&1 || _rc=$?
    if [ "${_rc}" -eq 0 ]; then
      printf '[smoke] pull %s 成功（第 %s 次尝试，累计用时 %ss）\n' \
        "$1" "${_attempt}" "$((SECONDS - _pull_start))" >>"$2"
      return 0
    fi
    # 「累计」是本次 pull 的总耗时，不是单次——排查时要能一眼分清「单次限时」和「总共烧了多久」。
    printf '[smoke] pull %s 失败（第 %s/%s 次，exit=%s，单次限时 %ss，累计用时 %ss）\n' \
      "$1" "${_attempt}" "${PULL_ATTEMPTS}" "${_rc}" "${_attempt_timeout}" \
      "$((SECONDS - _pull_start))" >>"$2"
    if [ "${_attempt}" -ge "${PULL_ATTEMPTS}" ]; then
      printf '[smoke] 已到次数上限 %s 次，停止重试\n' "${PULL_ATTEMPTS}" >>"$2"
      return 1
    fi
    if [ $((SECONDS + _interval)) -ge "${_pull_deadline}" ]; then
      printf '[smoke] 预算先到（已用 %ss，预算 %ss，下次间隔 %ss），停止重试\n' \
        "$((SECONDS - _pull_start))" "${PULL_BUDGET_SECONDS}" "${_interval}" >>"$2"
      return 1
    fi
    printf '[smoke] %ss 后重试\n' "${_interval}" >>"$2"
    sleep "${_interval}"
    _interval=$((_interval * 2))
    if [ "${_interval}" -gt "${PULL_MAX_INTERVAL_SECONDS}" ]; then
      _interval=${PULL_MAX_INTERVAL_SECONDS}
    fi
    _attempt=$((_attempt + 1))
  done
  return 1
}

wait_for_docker_build
# 拉取是否被前置条件挡住（构建没收敛）。非空即「不拉」，且原因一定写进 detail。
PULL_BLOCK_REASON=""
case "${WAIT_RESULT}" in
  ok | no-workflow) ;;
  timeout)
    WAIT_STATE=$(docker_build_state)
    PULL_BLOCK_REASON="docker.yml 在 ${WAIT_LIMIT_SECONDS}s 内未结束（${WAIT_STATE:-无记录}）"
    smoke_fail pull C "docker 镜像在验收窗口内构建完成" \
      "等了 ${WAIT_LIMIT_SECONDS}s，docker.yml 仍未结束（${WAIT_STATE:-无记录}）；如需放宽用 SMOKE_DOCKER_BUILD_WAIT_SECONDS（硬上限 SMOKE_DOCKER_TOTAL_WAIT_BUDGET_SECONDS）"
    ;;
  build-*)
    PULL_BLOCK_REASON="docker.yml 结论 ${WAIT_RESULT#build-}，镜像不会出现"
    smoke_fail pull C "docker 镜像构建成功" \
      "docker.yml 未成功（${WAIT_RESULT#build-}），镜像不会出现；先修镜像构建，不必改验收"
    ;;
  *)
    # 兜底分支：**绝不能**少。旧版没有它，一次「未识别的状态」就让整段拉取被静默跳过，
    # 结果格只剩「镜像没拉全」——这是 issue #14 假红的直接放大器。宁可照拉，多烧几分钟。
    smoke_warn "wait_for_docker_build 返回未识别状态 '${WAIT_RESULT}'，按「可拉」继续（驱动缺陷，已在 detail 记录）"
    smoke_log "WAIT_RESULT 未识别：${WAIT_RESULT:-空}"
    ;;
esac

PULL_VERSIONED_OK="no"
PULL_LATEST_OK="no"
if [ -n "${PULL_BLOCK_REASON}" ]; then
  smoke_log "跳过拉取：${PULL_BLOCK_REASON}（上面已记 C 类 fail，不重复记）"
else
  if pull_with_retry "${VERSIONED}" "${EVID}/pull-versioned.log"; then
    PULL_VERSIONED_OK="yes"
    smoke_pass pull "docker pull ${VERSIONED}"
  else
    smoke_fail pull C "docker pull ${VERSIONED}" "$(pull_fail_detail "${EVID}/pull-versioned.log")"
  fi
  if pull_with_retry "${LATEST}" "${EVID}/pull-latest.log"; then
    PULL_LATEST_OK="yes"
    smoke_pass pull "docker pull ${LATEST}"
  else
    smoke_fail pull C "docker pull ${LATEST}" "$(pull_fail_detail "${EVID}/pull-latest.log")"
  fi
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
  if [ -n "${PULL_BLOCK_REASON}" ]; then
    smoke_fail pull C ":${VERSION} 与 :latest 指向同一 digest" \
      "未拉取（${PULL_BLOCK_REASON}），无法比较 digest"
  else
    smoke_fail pull C ":${VERSION} 与 :latest 指向同一 digest" \
      "镜像没拉全，无法比较 digest（见上面 pull 格与 evidence/docker/pull-*.log）"
  fi
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

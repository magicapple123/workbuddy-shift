#!/usr/bin/env bash
# 把**需要人工介入**的失败格开成 issue（设计 §7.3）。
#
# 分工：类别 A–D 走自动处置（`dispose.py`），E–I 只能开 issue；`J`（blocked）**永不开
# issue**；无法归类的失败 fail-closed 也开 issue（宁可多一张，不可静默丢掉一个真失败）。
# 因此本脚本只读 `triage-plan.json` 里 `action == "issue"` 的项，外加 `dispose.json` 的
# `unresolved`（「处置没发出去」= 等于没处置，也要有人看）。
#
# 去重（设计 §7.3）：标题固定为 `[release-smoke] <tag> 验收失败：<形态> / <动作>`
# （由 `classify.py` 的 `issue_title()` 生成，**唯一真源**）：
#   - 已有同标题的 open issue → `gh issue comment` 追加，不新开；
#   - 没有 → `gh issue create`。
# 所以同一个 tag 连跑 3 次失败，最终只有 1 张 issue（T05 的验收标准 ③）。
#
# 硬约束：只读写 issue（`issues: write`）。不改源码、不动 tag / Release。
#
# 用法：
#   bash scripts/smoke/issue.sh --tag v0.2.5 --plan artifacts/release-smoke/triage-plan.json \
#       [--dispose artifacts/release-smoke/dispose.json] [--run-url https://…] [--dry-run]
#
# 退出码：0 = 已处理（含「无需开 issue」）；1 = gh 调用失败（triage 会显性化，但不阻断结论）。
set -eu

TAG=""
PLAN=""
DISPOSE=""
REPO=""
RUN_URL=""
VERSION=""
PREV=""
DRY_RUN=""

usage() {
  cat <<'USAGE'
用法：bash scripts/smoke/issue.sh --tag <vX.Y.Z> --plan <triage-plan.json>
                                  [--dispose <dispose.json>] [--repo owner/repo]
                                  [--run-url URL] [--version X.Y.Z] [--prev X.Y.Z] [--dry-run]

  --tag       失败对应的 Release tag（标题里带它，去重靠它）
  --plan      triage-plan.json（classify.py 产出）
  --dispose   dispose.json（可选）：其中的 unresolved 会被升级成 issue
  --run-url   本次 run 的链接（默认由 GITHUB_* 拼）
  --dry-run   只打印将要创建的 issue，不调用 gh
USAGE
}

while [ $# -gt 0 ]; do
  case "$1" in
    --tag) TAG=${2:-}; shift 2 ;;
    --plan) PLAN=${2:-}; shift 2 ;;
    --dispose) DISPOSE=${2:-}; shift 2 ;;
    --repo) REPO=${2:-}; shift 2 ;;
    --run-url) RUN_URL=${2:-}; shift 2 ;;
    --version) VERSION=${2:-}; shift 2 ;;
    --prev) PREV=${2:-}; shift 2 ;;
    --dry-run) DRY_RUN=1; shift ;;
    -h | --help) usage; exit 0 ;;
    *) printf '::warning::未知参数：%s\n' "$1" >&2; shift ;;
  esac
done

if [ -z "${TAG}" ] || [ -z "${PLAN}" ]; then
  printf '::error::issue.sh 需要 --tag 与 --plan\n' >&2
  exit 1
fi
if [ ! -f "${PLAN}" ]; then
  printf '::warning::计划文件不存在：%s（视为「无需开 issue」）\n' "${PLAN}" >&2
  exit 0
fi
if [ -z "${REPO}" ]; then
  REPO=${GH_REPO:-${GITHUB_REPOSITORY:-}}
fi
if [ -z "${RUN_URL}" ] && [ -n "${GITHUB_RUN_ID:-}" ]; then
  RUN_URL=$(printf '%s/%s/actions/runs/%s/attempts/%s' \
    "${GITHUB_SERVER_URL:-https://github.com}" "${GITHUB_REPOSITORY:-}" \
    "${GITHUB_RUN_ID}" "${GITHUB_RUN_ATTEMPT:-1}")
fi
VERSION=${VERSION:-$(printf '%s' "${TAG}" | sed -e 's/^[vV]//')}
PY=${SMOKE_PYTHON:-python3}

WORK=$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/release-issue.XXXXXX")
trap 'rm -rf "${WORK}"' EXIT

LABEL="release-smoke"

# 类别 → 人读含义（与 docs/release-smoke.md 的 A–J 表一致）。
category_label() {
  case "${1:-}" in
    A) printf 'A 瞬时失败' ;;
    B) printf 'B npm 平台包缺失' ;;
    C) printf 'C GHCR tag 缺失' ;;
    D) printf 'D Release 资产缺失' ;;
    E) printf 'E 安装失败' ;;
    F) printf 'F 启动即崩' ;;
    G) printf 'G 签名不匹配' ;;
    H) printf 'H 版本不一致' ;;
    I) printf 'I 容器不可用' ;;
    J) printf 'J 无法判定' ;;
    *) printf '未归类（fail-closed）' ;;
  esac
}

# 最小复现命令：按形态给**一条能直接跑**的命令（设计 §7.3 要求 issue 里必须有）。
repro_command() {
  _form=$1
  case "${_form}" in
    windows) printf 'pwsh -NoProfile -File scripts/smoke/win/driver.ps1 -Version %s -PrevVersion %s' "${VERSION}" "${PREV}" ;;
    macos) printf 'bash scripts/smoke/mac/driver.sh --version %s --prev %s' "${VERSION}" "${PREV}" ;;
    linux) printf 'bash scripts/smoke/linux/driver.sh --version %s --prev %s' "${VERSION}" "${PREV}" ;;
    npm) printf 'bash scripts/smoke/npm/driver.sh --version %s --prev %s --registry https://registry.npmjs.org' "${VERSION}" "${PREV}" ;;
    docker) printf 'bash scripts/smoke/docker/driver.sh --version %s --image %s' "${VERSION}" "${REPO:-ghcr.io/<owner>/<repo>}" ;;
    *) printf 'gh workflow run release-smoke.yml -f tag=%s -f dry_run=true' "${TAG}" ;;
  esac
}

# 从 plan / dispose 里抽出「要开 issue」的记录。
# 用 \x1f（unit separator）分隔字段：断言原文里出现制表符/竖线的概率远高于 \x1f。
extract_records() {
  "${PY}" - "${PLAN}" "${DISPOSE:-}" <<'PY'
import json
import sys

plan_path, dispose_path = sys.argv[1], sys.argv[2] if len(sys.argv) > 2 else ""
SEP = "\x1f"


def load(path):
    if not path:
        return None
    try:
        with open(path, encoding="utf-8") as handle:
            return json.load(handle)
    except Exception:  # noqa: BLE001 - 报告读不了不阻断（顶多少开一张 issue）
        return None


def emit(kind, item):
    fields = [
        kind,
        str(item.get("issueTitle") or ""),
        str(item.get("form") or "?"),
        str(item.get("cell") or "?"),
        str(item.get("category") or ""),
        str(item.get("severity") or "normal"),
        str(item.get("assertion") or ""),
        str(item.get("detail") or ""),
        str(item.get("reason") or ""),
        ",".join(str(e) for e in (item.get("evidence") or [])),
        str(item.get("logUrl") or ""),
    ]
    # 断言/详情里的换行会打断「一行一条记录」，压成空格（正文里仍可读）。
    print(SEP.join(field.replace("\n", " ").replace("\r", " ") for field in fields))


plan = load(plan_path) or {}
for item in plan.get("items") or []:
    if not isinstance(item, dict):
        continue
    if item.get("action") != "issue":
        continue  # J（blocked）/ 自动处置类不开 issue
    emit("plan", item)

report = load(dispose_path) or {}
for item in report.get("unresolved") or []:
    if not isinstance(item, dict):
        continue
    # 处置没发出去 = 没处置：升级成 issue，但标题沿用「形态/动作」，让它和真失败合并。
    item = dict(item)
    item.setdefault("assertion", "")
    item.setdefault("detail", "")
    item.setdefault("logUrl", "")
    item["issueTitle"] = item.get("issueTitle") or ""
    emit("unresolved", item)
PY
}

extract_records >"${WORK}/records.txt" 2>"${WORK}/extract.err" || {
  printf '::warning::抽取 issue 记录失败：%s\n' "$(cat "${WORK}/extract.err" 2>/dev/null || true)" >&2
  exit 1
}

if [ ! -s "${WORK}/records.txt" ]; then
  printf '[smoke] 没有需要开 issue 的格（全 pass / skipped / blocked，或处置已成功）\n'
  exit 0
fi

# label 不存在时 `gh issue create --label` 会直接失败：先幂等地建一次（不要求管理员权限）。
ensure_label() {
  _name=$1
  if [ -n "${REPO}" ]; then
    gh label create "${_name}" --description "release-smoke 验收相关" --color FBCA04 --force \
      --repo "${REPO}" >/dev/null 2>&1 || printf '::warning::创建 label %s 失败（已有则忽略）\n' "${_name}" >&2
  else
    gh label create "${_name}" --description "release-smoke 验收相关" --color FBCA04 --force \
      >/dev/null 2>&1 || printf '::warning::创建 label %s 失败（已有则忽略）\n' "${_name}" >&2
  fi
}

# 统一带 --repo（有的话）的 gh issue 调用；**不用** ${REPO:+--repo "$REPO"}——
# 那种写法依赖 word splitting，路径带空格时会静默拼错。
gh_issue() {
  if [ -n "${REPO}" ]; then
    gh issue "$@" --repo "${REPO}"
  else
    gh issue "$@"
  fi
}

if [ -z "${DRY_RUN}" ]; then
  ensure_label "${LABEL}"
fi

created=0
commented=0
failed=0

while IFS=$'\x1f' read -r kind title form cell category severity assertion detail reason evidence log_url; do
  # Windows 上 python 的 stdout 是 CRLF：`read` 只吃掉 \n，最后一列会带一个 \r。
  # 不清掉它，`[ -n "${log_url}" ]` 会判真，正文里就会出现一个空的 `[run 页面]()`。
  log_url=${log_url%$'\r'}
  # 标题为空就 continue 会把这一格**静默丢掉**（DEF-3）：dispose.json 的 unresolved
  # 过去没有 issueTitle，于是「处置没发出去」这一类失败永远不会变成 issue。
  # 兜底标题与 classify.py 的 issue_title() 同一格式（去重靠「标题里带 tag」）。
  if [ -z "${title}" ]; then
    title="[release-smoke] ${TAG} 验收失败：${form:-?} / ${cell:-?}"
    printf '::warning::记录缺少 issueTitle，已用兜底标题：%s\n' "${title}" >&2
  fi
  label_text=$(category_label "${category}")
  is_security=""
  if [ "${category}" = "G" ]; then
    is_security=1
    title="[security] ${title}"
  fi

  body="${WORK}/issue-body.md"
  {
    printf '## 摘要\n\n'
    printf -- '- 版本：`%s`（目标 `%s`）\n' "${TAG}" "${VERSION}"
    printf -- '- 平台 / 动作：`%s` / `%s`\n' "${form}" "${cell}"
    printf -- '- 类别：**%s** · 严重级：`%s`\n' "${label_text}" "${severity}"
    if [ -n "${log_url}" ]; then
      printf -- '- 日志：[run 页面](%s)\n' "${log_url}"
    fi
    printf '\n## 失败断言（原文）\n\n> %s\n' "${assertion:-（驱动脚本未记录断言原文）}"
    if [ -n "${detail}" ]; then
      printf '\n## 详情\n\n```\n%s\n```\n' "${detail}"
    fi
    if [ -n "${reason}" ]; then
      printf '\n## 判定理由\n\n%s\n' "${reason}"
    fi
    if [ -n "${evidence}" ]; then
      printf '\n## 证据\n\n'
      printf -- '- `%s`\n' ${evidence//,/ }
    fi
    printf '\n## 最小复现\n\n```bash\n%s\n```\n' "$(repro_command "${form}")"
    printf '\n## 处置\n\n'
    case "${kind}" in
      unresolved) printf -- '- 自动处置**未完成**（重跑/派发没有发出去），因此仍需人工介入\n' ;;
      *) printf -- '- 本次无自动动作（E–I 类不可自动修复，或归类失败）\n' ;;
    esac
    printf -- '- 历史：无历史（未做跨版本区块解析；如需对比请看上一版 Release 的 release-smoke 区块）\n'
    if [ -n "${RUN_URL}" ]; then
      printf -- '- run：%s\n' "${RUN_URL}"
    fi
    printf '\n<!-- release-smoke:issue kind=%s category=%s -->\n' "${kind}" "${category:-null}"
  } >"${body}"

  if [ -n "${DRY_RUN}" ]; then
    printf '\n===== [dry-run] 将创建/评论 issue：%s =====\n' "${title}"
    cat "${body}"
    continue
  fi

  # 去重：先找同标题的 open issue（标题里带 tag，所以同 tag 的多次失败会合并）。
  existing=$(gh_issue list --label "${LABEL}" --state open \
    --search "${TAG} in:title" --json number,title 2>/dev/null) || existing=""

  number=$(
    printf '%s' "${existing}" | "${PY}" -c '
import json, sys
wanted = sys.argv[1]
try:
    items = json.load(sys.stdin)
except Exception:
    items = []
for item in items if isinstance(items, list) else []:
    if str(item.get("title") or "") == wanted:
        print(item.get("number"))
        break
' "${title}" 2>/dev/null || true
  )

  if [ -n "${number}" ]; then
    if gh_issue comment "${number}" --body-file "${body}" >>"${WORK}/gh.log" 2>&1; then
      printf '[smoke] 已有 open issue #%s，已追加评论：%s\n' "${number}" "${title}"
      commented=$((commented + 1))
    else
      printf '::warning::追加评论失败：%s（见下方日志）\n' "${title}" >&2
      cat "${WORK}/gh.log" >&2 || true
      failed=$((failed + 1))
    fi
    continue
  fi

  labels="${LABEL}"
  if [ -n "${is_security}" ]; then
    ensure_label "security"
    labels="${LABEL},security"
  fi
  if gh_issue create --title "${title}" --body-file "${body}" --label "${labels}" >>"${WORK}/gh.log" 2>&1; then
    printf '[smoke] 已创建 issue：%s\n' "${title}"
    created=$((created + 1))
  else
    # label 组合失败时退一步：只带 release-smoke 再试一次（label 权限/不存在都常见）。
    if gh_issue create --title "${title}" --body-file "${body}" >>"${WORK}/gh.log" 2>&1; then
      printf '::warning::issue 创建成功但未带上 label（%s）\n' "${labels}" >&2
      created=$((created + 1))
    else
      printf '::warning::创建 issue 失败：%s\n' "${title}" >&2
      cat "${WORK}/gh.log" >&2 || true
      failed=$((failed + 1))
    fi
  fi
done <"${WORK}/records.txt"

printf '[smoke] issue 汇总：新建 %s · 评论 %s · 失败 %s\n' "${created}" "${commented}" "${failed}"
[ "${failed}" -eq 0 ] || exit 1
exit 0

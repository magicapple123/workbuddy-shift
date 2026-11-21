#!/usr/bin/env bash
# 把结论矩阵**幂等**写回 Release 正文的标记区块（设计 §7.1 / §7.2）。
#
# 为什么必须「读-剥-拼-写」而不是 append：`gh release edit --notes-file` 是**整篇替换**
# 语义。直接 append 会在每次重跑后堆一份新区块（历史事故），所以流程固定为：
#
#   1. `gh release view <tag> --json body` 读整篇正文；
#   2. `split-note.py --strip` 剥掉旧的 `<!-- release-smoke:begin -->…<!-- release-smoke:end -->`
#      （没有就原样返回，逐字保留发行说明）；
#   3. 把「新区块」（= 结论矩阵 + 首尾标记）拼到末尾；
#   4. `gh release edit <tag> --notes-file` 整篇写回。
#
# 幂等性：第 2 步之后正文里**一定没有**标记，所以重复执行只会替换、不会堆叠；
# `--attempts` 次重试时每次都**重新读**正文（`build.yml` 的 release job 可能同时在写，
# 拿旧 body 写回会把它的更新踩掉）。
#
# 硬约束（与设计 §1.2 一致）：不改源码、不改 tag、不动 `--draft` / `--latest` 状态、
# 不删 Release。只做 `gh release view` 与 `gh release edit`。
#
# 用法：
#   bash scripts/smoke/update-release-note.sh --tag v0.2.5 --conclusion artifacts/release-smoke/conclusion.md
#   bash scripts/smoke/update-release-note.sh --tag v0.2.5 --conclusion … --dry-run   # 只打印将写入的内容
#
# 退出码：0 = 写回成功（或 dry-run）；1 = 三次都失败（triage 会把它显性化，但不阻断结论）。
set -eu

TAG=""
REPO=""
CONCLUSION=""
DISPATCH=""
ATTEMPTS=3
DRY_RUN=""

usage() {
  cat <<'USAGE'
用法：bash scripts/smoke/update-release-note.sh --tag <vX.Y.Z> --conclusion <conclusion.md>
                                                 [--repo owner/repo] [--dispatch <dispose.json>]
                                                 [--attempts 3] [--dry-run]

  --tag         要写回的 Release tag
  --conclusion  结论矩阵 Markdown（render-conclusion.py --matrix 的 --out）
  --dispatch    dispose.json（可选）：把「本次处置 / 已抑制」写进区块
  --repo        owner/repo（默认读 GH_REPO / GITHUB_REPOSITORY）
  --attempts    重试次数（默认 3）
  --dry-run     只打印拼好的整篇正文，不调用 gh release edit
USAGE
}

while [ $# -gt 0 ]; do
  case "$1" in
    --tag) TAG=${2:-}; shift 2 ;;
    --conclusion) CONCLUSION=${2:-}; shift 2 ;;
    --dispatch) DISPATCH=${2:-}; shift 2 ;;
    --repo) REPO=${2:-}; shift 2 ;;
    --attempts) ATTEMPTS=${2:-3}; shift 2 ;;
    --dry-run) DRY_RUN=1; shift ;;
    -h | --help) usage; exit 0 ;;
    *) printf '::warning::未知参数：%s\n' "$1" >&2; shift ;;
  esac
done

if [ -z "${TAG}" ] || [ -z "${CONCLUSION}" ]; then
  printf '::error::update-release-note.sh 需要 --tag 与 --conclusion\n' >&2
  exit 1
fi
if [ ! -f "${CONCLUSION}" ]; then
  printf '::error::结论文件不存在：%s（triage 的 render 步骤没跑成？）\n' "${CONCLUSION}" >&2
  exit 1
fi
if [ -z "${REPO}" ]; then
  REPO=${GH_REPO:-${GITHUB_REPOSITORY:-}}
fi
PY=${SMOKE_PYTHON:-python3}
SPLIT_PY="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)/split-note.py"
if [ ! -f "${SPLIT_PY}" ]; then
  printf '::error::缺少 split-note.py：%s\n' "${SPLIT_PY}" >&2
  exit 1
fi

BEGIN_MARKER='<!-- release-smoke:begin -->'
END_MARKER='<!-- release-smoke:end -->'

WORK=$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/release-note.XXXXXX")
trap 'rm -rf "${WORK}"' EXIT

gh_release() {
  # $1=子命令（view / edit），其余参数原样透传；仓库可选。
  _gr_sub=$1
  shift
  if [ -n "${REPO}" ]; then
    gh release "${_gr_sub}" "$@" --repo "${REPO}"
  else
    gh release "${_gr_sub}" "$@"
  fi
}

# 「本次处置」行：从 dispose.json 里读，缺就写「无」（设计 §7.1 的区块内容要求）。
dispatch_line() {
  if [ -z "${DISPATCH}" ] || [ ! -f "${DISPATCH}" ]; then
    printf '**本次处置**：无（未执行 dispose，或 dry_run）\n'
    return 0
  fi
  "${PY}" - "${DISPATCH}" <<'PY'
import json
import sys

# 这一行会被原样写进 Release 正文：Windows 的文本模式会把 \n 翻成 \r\n，正文里就会多出
# 一个孤立 \r（DEF-5 的同一类问题，与 split-note.py 的 stdout 同理）。
try:
    sys.stdout.reconfigure(newline="\n")
except (AttributeError, ValueError, OSError):
    pass

try:
    with open(sys.argv[1], encoding="utf-8") as handle:
        report = json.load(handle)
except Exception as error:  # noqa: BLE001 - 报告读不了不该阻断写回
    print(f"**本次处置**：dispose.json 不可读（{error}）")
    raise SystemExit

if not isinstance(report, dict):
    print("**本次处置**：dispose.json 不是对象")
    raise SystemExit

executed = report.get("executed") or []
suppressed = report.get("suppressed") or []
unresolved = report.get("unresolved") or []
rejected = report.get("rejected") or []
prefix = "**本次处置（dry_run，未执行）**" if report.get("dryRun") else "**本次处置**"
parts = []
for record in executed:
    parts.append(f"`{record.get('category')}` {record.get('action')} {record.get('form')}")
line = prefix + "：" + (" · ".join(parts) if parts else "无自动处置")
bits = []
if suppressed:
    bits.append(f"已抑制 {len(suppressed)} 项")
if unresolved:
    bits.append(f"未完成 {len(unresolved)} 项（已升级为 issue）")
if rejected:
    bits.append(f"拒绝 {len(rejected)} 项（白名单外动作）")
if bits:
    line += "（" + "，".join(bits) + "）"
print(line)
PY
}

attempt=1
while [ "${attempt}" -le "${ATTEMPTS}" ]; do
  printf '[smoke] 写回 Release 正文：%s（第 %s/%s 次）\n' "${TAG}" "${attempt}" "${ATTEMPTS}" >&2
  if ! body=$(gh_release view "${TAG}" --json body --jq .body 2>"${WORK}/view.err"); then
    printf '::warning::gh release view 失败（第 %s 次）：%s\n' "${attempt}" "$(cat "${WORK}/view.err" 2>/dev/null || true)" >&2
    attempt=$((attempt + 1))
    sleep 2
    continue
  fi

  # 1) 剥掉旧区块（没有则逐字原样）；2) 拼新区块。
  printf '%s' "${body}" | "${PY}" "${SPLIT_PY}" --strip >"${WORK}/body.stripped.md" 2>"${WORK}/split.err" || {
    printf '::warning::split-note.py 失败（第 %s 次）：%s\n' "${attempt}" "$(cat "${WORK}/split.err" 2>/dev/null || true)" >&2
    attempt=$((attempt + 1))
    continue
  }

  {
    printf '%s\n' "${BEGIN_MARKER}"
    cat "${CONCLUSION}"
    printf '\n'
    dispatch_line
    printf '%s\n' "${END_MARKER}"
  } >"${WORK}/block.md"

  : >"${WORK}/notes.md"
  if [ -s "${WORK}/body.stripped.md" ]; then
    cat "${WORK}/body.stripped.md" >>"${WORK}/notes.md"
    printf '\n' >>"${WORK}/notes.md"
  fi
  cat "${WORK}/block.md" >>"${WORK}/notes.md"

  if [ -n "${DRY_RUN}" ]; then
    printf '[smoke] dry-run：将写入 %s 的正文如下（%s 字节）\n' "${TAG}" "$(wc -c <"${WORK}/notes.md")" >&2
    cat "${WORK}/notes.md"
    exit 0
  fi

  if gh_release edit "${TAG}" --notes-file "${WORK}/notes.md" >"${WORK}/edit.out" 2>"${WORK}/edit.err"; then
    printf '[smoke] 已写回 %s（区块首行：%s）\n' "${TAG}" "$(head -1 "${CONCLUSION}")"
    exit 0
  fi

  printf '::warning::gh release edit 失败（第 %s 次）：%s\n' "${attempt}" "$(cat "${WORK}/edit.err" 2>/dev/null || true)" >&2
  attempt=$((attempt + 1))
  sleep 2
done

printf '::error::写回 Release 正文失败（已重试 %s 次）：%s\n' "${ATTEMPTS}" "${TAG}" >&2
exit 1

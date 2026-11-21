#!/usr/bin/env bash
# 读 Linux deb 形态的包名 / 已安装版本 / 主程序路径，输出一行 JSON。
#
# 四种用法：
#   bash scripts/smoke/linux/read-version.sh --deb workbuddy-shift-linux-x86_64.deb
#       → 从**包元数据**读 `Package` / `Version`，并从包内容清单选主程序（安装前的期望值）
#   bash scripts/smoke/linux/read-version.sh --package workbuddy-shift
#       → 从 `dpkg-query` 读已安装版本，并用 `dpkg -L` 的清单选主程序
#   bash scripts/smoke/linux/read-version.sh --from-list files.txt   # `-` = stdin
#       → 只做「从文件清单选主程序」这一步（可喂 `dpkg -L` 或
#         `ar p x.deb data.tar.gz | tar -tzf -` 的输出），离线可验证
#   bash scripts/smoke/linux/read-version.sh --selftest
#       → 用固定的文件清单 / 临时的 `.desktop` 目录断言选择规则（无 dpkg / 无网络）
#
# 输出：`{"package":"...","version":"...","binary":"...","installed":true|false,"ok":true|false,"error":""}`
#
# 主程序为什么不能取「`dpkg -L` 里的第一个 `/usr/bin/`」
# ---------------------------------------------------
# deb 里有两个可执行文件：`/usr/bin/companion-runtime`（悬浮窗 sidecar）与
# `/usr/bin/workbuddy-shift-desktop`（Tauri 桌面壳）。`dpkg -L` 的输出按字典序，
# `companion-runtime` 排在前，于是 driver 拿 sidecar 当主程序启动 → 20s 内退出 → 记 F。
# 那是**选错了验证对象**，不是产品起不来（run 44941972411 的 startup=F；同一台的
# AppImage 启动格是 pass）。所以选择顺序固定为：
#
#   1. `<root>/usr/share/applications/*.desktop` 的 `Exec=`：这是**桌面环境的权威口径**
#      ——用户在菜单里点的就是它。去掉 `%U` / `%F` 这类占位符，取第一个 token，解析成绝对路径
#      （先在清单里按 basename 精确匹配，真实系统上再退回 `command -v`）；
#   2. `/usr/bin/` 清单**排除** `*companion*` 后，优先 `*-desktop`，其次含包名 slug 的条目；
#   3. 都不匹配 → `ok=false` + `error` 写明「未能确定桌面主程序」，**绝不硬选**
#      （硬选一个 companion-* 会让 driver 的启动格假装成产品缺陷）。
#
# 约定：
# - **恒 exit 0**（`--selftest` 例外：断言失败 exit 1）：读不出来就是 `ok=false` + `error`，
#   判定交给 driver；
# - 包名一律从 deb 元数据或 `dpkg-query` 拿，**不写死**。

set -eu

MODE=""
TARGET=""
# 文档根：真实运行时是 `/`；`--selftest` 指到临时目录，好让 `.desktop` 读取也离线可验证。
ROOT="/"

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
    --from-list)
      MODE="list"
      TARGET=${2:-}
      shift 2
      ;;
    --root)
      ROOT=${2:-/}
      shift 2
      ;;
    --selftest)
      MODE="selftest"
      shift
      ;;
    -h | --help)
      sed -n '2,30p' "$0"
      exit 0
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

# ---------------------------------------------------------------------------
# 主程序选择（纯文本规则，不依赖 dpkg）
# ---------------------------------------------------------------------------

# 「多值输出」走全局变量：`$(...)` 是子 shell，函数里选中的结果会丢。
SELECTED=""
SELECTED_VIA=""
SELECT_ERROR=""

reset_selection() {
  SELECTED=""
  SELECTED_VIA=""
  SELECT_ERROR=""
}

is_companion() {
  # sidecar（悬浮窗 companion-runtime）不是桌面主程序。
  case "$(basename -- "${1:-}")" in
    *companion*) return 0 ;;
    *) return 1 ;;
  esac
}

normalize_list_line() {
  # 统一成绝对路径：`dpkg -c` 与 `tar -tzf` 打 `./usr/bin/x`，`dpkg -L` 打 `/usr/bin/x`。
  # 目录条目（带尾斜杠）与空行丢弃。
  _nl=${1:-}
  _nl=$(printf '%s' "${_nl}" | tr -d '\r' | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')
  [ -n "${_nl}" ] || return 0
  case "${_nl}" in
    */) return 0 ;;
  esac
  _nl=${_nl#./}
  case "${_nl}" in
    /*) ;;
    *) _nl="/${_nl}" ;;
  esac
  printf '%s\n' "${_nl}"
}

# 候选条目是不是「可执行文件」。
# `dpkg -L` 会把目录也打出来（`/usr/bin` 这类**没有**尾斜杠），不排除就会选出 `/usr/bin`
# 本身。文件系统能回答时以它为准（真实运行）；离线清单（`--from-list` / 自测）里文件
# 不存在，退化为按名字排除明显的目录名。
keep_candidate() {
  _kc_path=${1:-}
  _kc_root=${2:-/}
  _kc_abs="${_kc_root%/}${_kc_path}"
  if [ -e "${_kc_abs}" ]; then
    if [ -f "${_kc_abs}" ] && [ -x "${_kc_abs}" ]; then return 0; fi
    return 1
  fi
  case "$(basename -- "${_kc_path}")" in
    bin | sbin | lib | lib64 | share | doc | applications | include | etc | var | usr)
      return 1
      ;;
  esac
  return 0
}

# 从 `.desktop` 里取 `Exec=` 的第一个非占位符 token（可能是绝对路径，也可能是裸命令名）。
desktop_exec_of() {
  _de_file=${1:-}
  [ -f "${_de_file}" ] || return 0
  _de_line=$(grep -m1 -E '^[[:space:]]*Exec[[:space:]]*=' -- "${_de_file}" 2>/dev/null || true)
  [ -n "${_de_line}" ] || return 0
  _de_value=${_de_line#*=}
  printf '%s\n' "${_de_value}" | awk '{
    for (i = 1; i <= NF; i++) {
      token = $i
      gsub(/%[A-Za-z]/, "", token)   # 去掉 %U / %F / %f / %u / %k / %i
      if (token != "") { print token; exit }
    }
  }'
}

# 把一个 `Exec=` token 解析成绝对路径（清单优先，真实系统上再退回 PATH）。
resolve_exec_token() {
  _rt=${1:-}
  _rt_list=${2:-}
  [ -n "${_rt}" ] || return 0
  case "${_rt}" in
    */*)
      printf '%s\n' "${_rt}"
      return 0
      ;;
  esac
  # 裸命令名：先在文件清单里按 basename 精确匹配（离线也可验证），/usr/bin 优先。
  for _rt_prefix in /usr/bin/ /usr/local/bin/ /bin/ /opt/; do
    _rt_hit=$(printf '%s\n' "${_rt_list}" | awk -v tok="${_rt}" -v pre="${_rt_prefix}" '
      index($0, pre) == 1 { n = split($0, p, "/"); if (p[n] == tok) { print; exit } }')
    if [ -n "${_rt_hit}" ]; then
      printf '%s\n' "${_rt_hit}"
      return 0
    fi
  done
  # 清单里没有（例如 `Exec` 指的是 /usr/local/bin 下的包装脚本）：退回 PATH。
  # 但**排除包装器**——`Exec=env X=1 foo` 会把 `env` 解析成 /usr/bin/env，那就是第二个
  # 「选错对象」的坑。
  _rt_cmd=$(command -v "${_rt}" 2>/dev/null || true)
  [ -n "${_rt_cmd}" ] || return 0
  case "$(basename -- "${_rt_cmd}")" in
    env | sh | bash | dash | zsh | python | python3 | perl | node)
      return 0
      ;;
  esac
  printf '%s\n' "${_rt_cmd}"
}

# 选择主程序：$1 = 规范化后的文件清单（多行） $2 = 文档根 $3 = 包名/slug（可空）
# 结果放在 SELECTED / SELECTED_VIA / SELECT_ERROR。
select_binary() {
  _sb_list=${1:-}
  _sb_root=${2:-/}
  _sb_slug=${3:-}
  reset_selection

  # 1) 桌面入口（权威口径）
  for _sb_desktop in "${_sb_root%/}/usr/share/applications"/*.desktop; do
    [ -f "${_sb_desktop}" ] || continue
    _sb_token=$(desktop_exec_of "${_sb_desktop}")
    [ -n "${_sb_token}" ] || continue
    _sb_resolved=$(resolve_exec_token "${_sb_token}" "${_sb_list}")
    [ -n "${_sb_resolved}" ] || continue
    if is_companion "${_sb_resolved}"; then
      continue
    fi
    SELECTED=${_sb_resolved}
    SELECTED_VIA="desktop:$(basename -- "${_sb_desktop}")"
    return 0
  done

  # 2) /usr/bin 清单启发式（排除 companion-* 与不可执行条目）
  _sb_all_bins=""
  _sb_cands=""
  while IFS= read -r _sb_line; do
    [ -n "${_sb_line}" ] || continue
    case "${_sb_line}" in
      /usr/bin/?*) ;;
      *) continue ;;
    esac
    _sb_all_bins="${_sb_all_bins}${_sb_line}
"
    keep_candidate "${_sb_line}" "${_sb_root}" || continue
    if is_companion "${_sb_line}"; then continue; fi
    _sb_cands="${_sb_cands}${_sb_line}
"
  done <<EOF
${_sb_list}
EOF

  # a) 以 -desktop 结尾（Tauri 的 productName → 桌面壳名）
  _sb_pick=$(printf '%s' "${_sb_cands}" | awk '{
    n = split($0, p, "/")
    if (p[n] ~ /-desktop$/) { print; exit }
  }')
  # b) 含包名 slug（包名 → 可执行名一般一致）
  if [ -z "${_sb_pick}" ] && [ -n "${_sb_slug}" ]; then
    _sb_pick=$(printf '%s' "${_sb_cands}" | awk -v slug="${_sb_slug}" 'index($0, slug) > 0 { print; exit }')
  fi
  # c) 任何一个非 companion 的条目
  if [ -z "${_sb_pick}" ]; then
    _sb_pick=$(printf '%s' "${_sb_cands}" | awk 'NF { print; exit }')
  fi
  if [ -n "${_sb_pick}" ]; then
    SELECTED=${_sb_pick}
    SELECTED_VIA="usr-bin"
    return 0
  fi

  # 3) 都匹配不上：如实说明，不硬选
  if [ -n "${_sb_all_bins}" ]; then
    SELECT_ERROR="未能确定桌面主程序：/usr/bin 下只有 companion-*（$(printf '%s' "${_sb_all_bins}" | tr '\n' ' ')）；请检查 deb 的 .desktop 或可执行文件命名"
  else
    SELECT_ERROR="未能确定桌面主程序：文件清单里没有 /usr/bin/ 条目"
  fi
  return 0
}

# 从 stdin 读一份文件清单并规范化（`--from-list -` / `dpkg -L` / `tar -tzf` 的输出）
read_list_stdin() {
  while IFS= read -r _rl_line || [ -n "${_rl_line}" ]; do
    normalize_list_line "${_rl_line}"
  done
}

# ---------------------------------------------------------------------------
# 自测：固定的文件清单 + 临时 `.desktop`，断言选择规则（无 dpkg / 无网络）
# ---------------------------------------------------------------------------
selftest() {
  _st_fail=0
  _st_tmp=$(mktemp -d "${TMPDIR:-/tmp}/smoke-read-version.XXXXXX")
  # shellcheck disable=SC2064  # 变量此刻展开，正是想要的值
  trap "rm -rf '${_st_tmp}'" EXIT
  mkdir -p "${_st_tmp}/usr/share/applications" "${_st_tmp}/usr/bin"

  # $1=用例名 $2=期望选中 $3=期望 via（空=不校验） $4=清单 $5=文档根 $6=slug
  _st_case() {
    select_binary "${4}" "${5}" "${6:-}"
    _st_ok="yes"
    if [ "${SELECTED}" != "${2}" ]; then _st_ok="no"; fi
    if [ -n "${3:-}" ] && [ "${SELECTED_VIA}" != "${3}" ]; then _st_ok="no"; fi
    if [ "${_st_ok}" = "yes" ]; then
      printf 'PASS  %s → binary=%s via=%s\n' "${1}" "${SELECTED:-—}" "${SELECTED_VIA:-—}"
    else
      _st_fail=$((_st_fail + 1))
      printf 'FAIL  %s → binary=%s（期望 %s）via=%s（期望 %s）\n' \
        "${1}" "${SELECTED:-—}" "${2:-—}" "${SELECTED_VIA:-—}" "${3:-任意}"
    fi
  }

  # 真实 deb 的清单（run 44941972411 现场：两个可执行文件 + 一个桌面入口）
  _st_real='/usr/bin
/usr/bin/companion-runtime
/usr/bin/workbuddy-shift-desktop
/usr/share/applications
/usr/share/applications/workbuddy-shift-desktop.desktop'

  # 同样的清单，但没有 `.desktop`（模拟缺桌面入口）
  _st_no_desktop='/usr/bin
/usr/bin/companion-runtime
/usr/bin/workbuddy-shift-desktop'

  # 有桌面入口（文档根里放一份真实的 .desktop）
  cat >"${_st_tmp}/usr/share/applications/workbuddy-shift-desktop.desktop" <<'DESKTOP'
[Desktop Entry]
Type=Application
Name=WorkBuddy Shift
Comment=WorkBuddy 多账号切换
Exec=/usr/bin/workbuddy-shift-desktop %U
Icon=workbuddy-shift
Categories=Utility;
DESKTOP
  _st_case "真实 deb 清单 + 桌面入口 → 取 Exec 的绝对路径" \
    "/usr/bin/workbuddy-shift-desktop" "desktop:workbuddy-shift-desktop.desktop" \
    "${_st_real}" "${_st_tmp}" "workbuddy-shift"

  # Exec 只写裸命令名（常见写法）→ 仍需解析成清单里的 /usr/bin/...
  cat >"${_st_tmp}/usr/share/applications/workbuddy-shift-desktop.desktop" <<'DESKTOP'
[Desktop Entry]
Type=Application
Name=WorkBuddy Shift
Exec=workbuddy-shift-desktop %F
DESKTOP
  _st_case "Exec 是裸命令名 → 在清单里按 basename 解析" \
    "/usr/bin/workbuddy-shift-desktop" "desktop:workbuddy-shift-desktop.desktop" \
    "${_st_real}" "${_st_tmp}" "workbuddy-shift"

  # 没有桌面入口 → 清单启发式：排除 companion-*，优先 *-desktop
  rm -f "${_st_tmp}/usr/share/applications/workbuddy-shift-desktop.desktop"
  _st_case "无 .desktop + 清单里 companion 排在前 → 仍选 *-desktop" \
    "/usr/bin/workbuddy-shift-desktop" "usr-bin" \
    "${_st_no_desktop}" "${_st_tmp}" "workbuddy-shift"

  _st_case "无 .desktop + 清单里混入更靠前的非 companion 条目 → 优先 *-desktop" \
    "/usr/bin/workbuddy-shift-desktop" "usr-bin" \
    '/usr/bin/aaa-helper
/usr/bin/workbuddy-shift-desktop
/usr/bin/companion-runtime' "${_st_tmp}" "workbuddy-shift"

  # 只有 companion-runtime → 必须报错（过去会硬选它，伪装成启动失败）
  select_binary '/usr/bin/companion-runtime
/usr/bin/companion-ui' "${_st_tmp}" "workbuddy-shift"
  if [ -z "${SELECTED}" ] && [ -n "${SELECT_ERROR}" ]; then
    printf 'PASS  只有 companion-* → 不硬选，error=%s\n' "${SELECT_ERROR}"
  else
    _st_fail=$((_st_fail + 1))
    printf 'FAIL  只有 companion-* → 仍选中 %s（error=%s）\n' "${SELECTED:-—}" "${SELECT_ERROR:-—}"
  fi

  # 空清单 → 也要报错
  select_binary "" "${_st_tmp}" "workbuddy-shift"
  if [ -z "${SELECTED}" ] && [ -n "${SELECT_ERROR}" ]; then
    printf 'PASS  空清单 → error=%s\n' "${SELECT_ERROR}"
  else
    _st_fail=$((_st_fail + 1))
    printf 'FAIL  空清单 → 选中 %s\n' "${SELECTED:-—}"
  fi

  # 清单规范化：`dpkg -c` / `tar -tzf` 的 `./usr/bin/x` 与目录条目
  _st_norm=$(printf '%s\n' './usr/bin/' './usr/bin/workbuddy-shift-desktop' 'usr/bin/companion-runtime' | read_list_stdin)
  _st_expected='/usr/bin/workbuddy-shift-desktop
/usr/bin/companion-runtime'
  if [ "${_st_norm}" = "${_st_expected}" ]; then
    printf 'PASS  清单规范化（./ 前缀 / 目录条目）\n'
  else
    _st_fail=$((_st_fail + 1))
    printf 'FAIL  清单规范化 → %s\n' "$(printf '%s' "${_st_norm}" | tr '\n' ' ')"
  fi

  if [ "${_st_fail}" -gt 0 ]; then
    printf 'self-test 失败：%d 项\n' "${_st_fail}"
    return 1
  fi
  printf 'self-test passed\n'
  return 0
}

# ---------------------------------------------------------------------------
# 主流程
# ---------------------------------------------------------------------------

if [ "${MODE}" = "selftest" ]; then
  selftest
  exit $?
fi

if [ -z "${MODE}" ]; then
  emit "" "" "" false false "缺少 --deb / --package / --from-list 参数"
  exit 0
fi

PKG=""
VERSION=""
BINARY=""
INSTALLED="false"
ERROR=""

# 只做「从文件清单选主程序」
if [ "${MODE}" = "list" ]; then
  if [ -z "${TARGET}" ]; then
    emit "" "" "" false false "缺少 --from-list 的清单路径（- 表示 stdin）"
    exit 0
  fi
  if [ "${TARGET}" != "-" ] && [ ! -f "${TARGET}" ]; then
    emit "" "" "" false false "清单文件不存在：${TARGET}"
    exit 0
  fi
  if [ "${TARGET}" = "-" ]; then
    _list_text=$(read_list_stdin)
  else
    _list_text=$(read_list_stdin <"${TARGET}")
  fi
  select_binary "${_list_text}" "${ROOT}" ""
  BINARY=${SELECTED}
  OK="true"
  if [ -z "${BINARY}" ]; then
    OK="false"
    ERROR=${SELECT_ERROR:-"未能确定桌面主程序"}
  fi
  emit "" "" "${BINARY}" false "${OK}" "${ERROR}"
  exit 0
fi

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
  # 安装前就把「装完该启动哪个可执行文件」算出来：包内容清单走同一套选择规则。
  _deb_list=$(dpkg-deb -c "${TARGET}" 2>/dev/null | awk '{ print $NF }' | read_list_stdin || true)
  select_binary "${_deb_list}" "${ROOT}" "${PKG}"
  BINARY=${SELECTED}
  ERROR=${SELECT_ERROR}
  OK="true"
  if [ -z "${PKG}" ] || [ -z "${VERSION}" ]; then
    OK="false"
    ERROR="dpkg-deb 读不到 Package/Version"
  elif [ -z "${BINARY}" ]; then
    OK="false"
  fi
  emit "${PKG}" "${VERSION}" "${BINARY}" "${INSTALLED}" "${OK}" "${ERROR}"
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
_pkg_list=$(dpkg -L "${PKG}" 2>/dev/null | read_list_stdin || true)
select_binary "${_pkg_list}" "${ROOT}" "${PKG}"
BINARY=${SELECTED}
if [ -n "${SELECT_ERROR}" ]; then
  if [ -n "${ERROR}" ]; then
    ERROR="${ERROR}；${SELECT_ERROR}"
  else
    ERROR="${SELECT_ERROR}"
  fi
fi

OK="true"
if [ "${INSTALLED}" != "true" ] || [ -z "${BINARY}" ]; then
  OK="false"
fi
emit "${PKG}" "${VERSION}" "${BINARY}" "${INSTALLED}" "${OK}" "${ERROR}"
exit 0

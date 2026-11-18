//! 限额 hook 的安装 / 卸载 / 状态：把 `Stop` + `FinalStop` 两个事件注册进
//! 三处客户端配置（`~/.codebuddy`、`~/.workbuddy`、`~/.workbuddy-ai` 的 `settings.json`）。
//!
//! 只注册这两个事件（三轮探针实测：CLI / WorkBuddy 的 429 当轮触发 `Stop`，
//! `FinalStop` 是终态补充；`PreToolUse` / `PostToolUse` 之类会随每次工具调用触发，
//! 只增加客户端开销）。hook 脚本把 stdin payload 追加到 `~/.workbuddy-shift/hook-events.jsonl`，
//! 由后端消费（见 `rate_limit_events.rs`）。
//!
//! 三条硬约束（安装会改写用户真实配置，必须守住）：
//! - **幂等**：重复安装不产生重复条目；
//! - **写前备份**：安装前把原文件原样复制到 `~/.workbuddy-shift/hook-backups/`；
//! - **可还原**：卸载时若配置的语义与备份一致（除本工具的条目外没有别的改动），
//!   直接把备份字节写回，做到逐字节还原；用户后来改过别的键时只做结构化移除，不覆盖用户改动。
//!
//! 全部公开入口都通过 [`HookLayout`] 取路径，安装 / 卸载 / 状态的核心实现接收显式 layout，
//! 单测一律注入临时目录，不触碰真实 `~/.codebuddy`、`~/.workbuddy`、`~/.workbuddy-ai`、`~/.workbuddy-shift`。
//!
//! 存在性判据（追加需求）：客户端数据根目录 `is_dir()` 是唯一判据。不存在的数据根不写配置、
//! 不为其创建目录 / `settings.json`；一处客户端都没装时安装是「什么都不做」，不是错误。

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::modules::config::{self, atomic_write, home_dir};

/// hook 脚本名（全平台统一 sh，见 `script_name`）。
const SCRIPT_NAME_SH: &str = "hook.sh";
/// hook 事件信号文件：脚本 append、后端消费。
pub const EVENTS_FILE_NAME: &str = "hook-events.jsonl";
/// 客户端设置文件名（三处配置同名）。
pub(crate) const SETTINGS_FILE_NAME: &str = "settings.json";
/// 安装前备份目录（卸载时据此逐字节还原）。
const BACKUP_DIR_NAME: &str = "hook-backups";

/// 注册的事件（最小集）。
const HOOK_EVENTS: [&str; 2] = ["Stop", "FinalStop"];

/// 客户端数据根目录名 → 备份文件名标签（三处配置的稳定标识）。
const TARGETS: [(&str, &str); 3] = [
    (".codebuddy", "codebuddy"),
    (".workbuddy", "workbuddy"),
    (".workbuddy-ai", "workbuddy-ai"),
];

/// 本应用的数据目录名（与 `config::store_dir()` 一致）。
const STORE_DIR_NAME: &str = ".workbuddy-shift";

// ---------------------------------------------------------------------------
// 平台脚本
// ---------------------------------------------------------------------------

/// 旧版 Windows 脚本名（`cmd /c` 形态，2026-09-18 弃用；安装 / 卸载时顺带清理其注册条目）。
const LEGACY_CMD_NAME: &str = "hook.cmd";

/// 模板骨架：`script_body` 里除「转义后的事件文件路径」之外的全部内容。
///
/// 用来认出「同代模板、只是把事件写到了别处」的脚本（见 `template_events_target`）。
const SCRIPT_PREFIX: &str = "#!/bin/sh\npayload=$(cat)\nprintf '%s\\n' \"$payload\" >> ";

const SCRIPT_SUFFIX: &str = "\nprintf '{}'\n";

/// 上一代数据目录名（本产品改名前的用户目录）。
///
/// 只用于**识别与清理**上一代留下的注册条目：那条命令指向旧目录里的 `hook.sh`，升级后会被
/// 当成第三方条目原样保留 ⇒ 双触发，且事件继续写进已废弃的目录。2026-11-17 本机实证：
/// 三处 `settings.json` 只剩旧条目、新脚本又被误判成「用户改过」，于是 hook 从未装上，
/// 限额只能靠日志扫描兜底 ⇒ 卡片缺 chip、模型名归因失败。
///
/// 边界：不改名、不迁移旧目录里的用户数据、不删除旧目录里的文件；只把指向它的注册条目
/// 换成本工具当前的形态（登记见 AGENTS.md 第 7 节）。
const LEGACY_STORE_DIRS: [&str; 1] = [".wb-switch"];

/// 已知的旧版 sh 脚本正文（绝对路径化之前、依赖 `$HOME` 的那一份）。
///
/// 只用于**识别**：内容与它逐字节一致才允许升级重写 / 卸载删除。
const LEGACY_SH_BODY: &str = "#!/bin/sh\npayload=$(cat)\nprintf '%s\\n' \"$payload\" >> \"$HOME/.workbuddy-shift/hook-events.jsonl\"\nprintf '{}'\n";

/// 已知的旧版 cmd 脚本正文（Windows 的 `cmd` + PowerShell 形态）。
///
/// 当时正文里不含任何待替换的绝对路径，因此逐字节匹配即可认出。
const LEGACY_CMD_BODY: &str = concat!(
    "@echo off\r\n",
    "powershell -NoProfile -ExecutionPolicy Bypass -Command \"$d=[Console]::In.ReadToEnd(); ",
    "if ($d.Trim().Length -gt 0) { Add-Content -LiteralPath (Join-Path $env:USERPROFILE '.workbuddy-shift\\hook-events.jsonl') ",
    "-Value $d.TrimEnd() -Encoding UTF8 }\"\r\n",
    "echo {}\r\n",
);

/// hook 脚本名：**全平台统一 sh**。Windows 的执行器是客户端自带的 PortableGit bash
/// （与插件 hook 同款调用），实测 `cmd /c "..."` 在该环境里静默失败 —— spawn 成功但
/// 批处理零副作用（2026-09-18 探针实证：连 `%TEMP%` 的 mkdir 都没发生），而
/// `bash "<路径>"` 形式的插件 hook 一直正常。
fn script_name() -> &'static str {
    SCRIPT_NAME_SH
}

/// hook 脚本正文。
///
/// ⚠️ 事件文件路径在安装时**写死为绝对路径**，不依赖 `HOME` / `USERPROFILE`：
/// hook 由各客户端自己的执行器拉起，运行环境不可控，环境变量并不可靠；
/// 安装时一次解析，之后逐字节幂等。路径统一经 [`shell_quote`] 转义：含空格 / 中文 /
/// 单引号的合法路径都不能破坏引号配对（2026-09-20 审查实证：裸拼单引号会让
/// `o'brien` 这类路径静默丢事件 —— stderr 报错、退出码 0、stdout 仍是 `{}`）。
///
/// 一次 `cat` 读入 payload、一次 `printf` 原样追加（尽量单次 write，减少并发追加的行内
/// 交错），最后必须回 `{}`——空 stdout 会被客户端当作 hook 失败。
fn script_body(events: &Path) -> String {
    // Windows 下统一正斜杠（MSYS bash 对 `C:/...` 原生支持；反斜杠在引号里是转义雷区）。
    let sh_events = events.to_string_lossy().replace('\\', "/");
    [
        "#!/bin/sh",
        "payload=$(cat)",
        &format!("printf '%s\\n' \"$payload\" >> {}", shell_quote(&sh_events)),
        "printf '{}'",
        "",
    ]
    .join("\n")
}

/// MSYS 风格路径：`C:\a\b` → `/c/a/b`（PortableGit bash 的原生形态，与插件 hook 一致）。
fn msys_path(script: &Path) -> String {
    let s = script.to_string_lossy().replace('\\', "/");
    let mut chars = s.chars();
    let drive = chars.next().unwrap_or('c').to_ascii_lowercase();
    match chars.next() {
        Some(':') => format!("/{}{}", drive, chars.as_str()),
        _ => s,
    }
}

/// 注册命令里的脚本路径**实参形态**：Windows 走 MSYS 路径，其余平台原样。
///
/// marker 与注册命令都用它拼装，保证两侧逐字节一致。
fn script_arg(script: &Path) -> String {
    if cfg!(windows) {
        msys_path(script)
    } else {
        script.to_string_lossy().to_string()
    }
}

/// 单引号包裹 shell 参数（路径可能含空格 / 中文 / 单引号）。
///
/// 单引号按 POSIX 写法转义为 `'\''`：这是 marker 与注册命令共用的唯一形态。
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// 解开 [`shell_quote`] 的单引号包裹；不是该形态返回 `None`。
fn shell_unquote(value: &str) -> Option<String> {
    let inner = value.strip_prefix('\'')?.strip_suffix('\'')?;
    Some(inner.replace("'\\''", "'"))
}

/// 重定向目标是否落在**事件文件**上（`…/hook-events.jsonl`）。
///
/// 只认「目录分隔符 + 文件名」：`not-hook-events.jsonl` 这类同名尾巴不算。
fn is_events_target(path: &str) -> bool {
    path.strip_suffix(EVENTS_FILE_NAME)
        .is_some_and(|dir| dir.ends_with('/') || dir.ends_with('\\'))
}

/// 内容是否是「本工具模板的某一份」——骨架逐字节一致，只有事件文件路径不同。
///
/// 历史上本工具的正文有两种书写，都从同一份模板渲染而来：
/// - `'<绝对路径>/hook-events.jsonl'`（当前形态，单引号）；
/// - `"$HOME/<目录>/hook-events.jsonl"`（绝对路径化之前的两代，双引号）。
///
/// 命中即说明这份脚本由本工具生成、只是把事件写到了别的目录（上一代数据目录 / 改名前的
/// 用户目录）：升级重写、卸载删除都安全。认不出（骨架不一致、目标不是事件文件、双引号形态
/// 却不是 `$HOME/` 打头）返回 `None`，调用方按「用户改过」处理，既不覆盖也不删除。
///
/// 返回值是脚本里**原样书写**的事件路径（双引号形态保留 `$HOME/` 前缀）。
fn template_events_target(content: &str) -> Option<String> {
    let rest = content.strip_prefix(SCRIPT_PREFIX)?;
    let quoted = rest.strip_suffix(SCRIPT_SUFFIX)?;
    let target = if quoted.starts_with('\'') {
        shell_unquote(quoted)?
    } else {
        let inner = quoted.strip_prefix('"')?.strip_suffix('"')?;
        if !inner.starts_with("$HOME/") {
            return None;
        }
        inner.to_string()
    };
    is_events_target(&target).then_some(target)
}

/// hook 脚本的启动命令（写进三处 `settings.json` 的 `command`）。
fn hook_command(script: &Path) -> String {
    let interpreter = if cfg!(windows) { "bash" } else { "sh" };
    format!("{interpreter} {}", shell_quote(&script_arg(script)))
}

// ---------------------------------------------------------------------------
// 目标布局
// ---------------------------------------------------------------------------

/// 一处客户端配置。
struct HookTarget {
    label: &'static str,
    /// 客户端数据根目录：**唯一**的存在性判据（`settings.json` 可能存在也可能还没写过）。
    data_root: PathBuf,
    settings: PathBuf,
}

impl HookTarget {
    /// 本机是否装了该客户端（数据根目录存在）。
    fn exists(&self) -> bool {
        self.data_root.is_dir()
    }

    /// 该处配置里是否已注册本工具 hook。
    fn registered(&self, marker: &str) -> bool {
        settings_has_marker(&self.settings, marker)
    }
}

/// 既有 hook 脚本的归属：决定升级与卸载时动不动这个文件。
enum ScriptOwnership {
    /// 脚本不存在（全新接入）。
    Absent,
    /// 与当前模板逐字节一致：不需要任何动作。
    Current,
    /// 本工具的已知旧模板：升级时重写、卸载时删除都安全。
    ///
    /// 三种形态：`LEGACY_SH_BODY`（`$HOME` + 本工具数据目录的最早一版）、同代模板但事件文件
    /// 指向别处（上一代数据目录，见 `template_events_target`）的两种书写（单引号绝对路径 /
    /// 双引号 `$HOME`）。
    Outdated,
    /// 内容未知（用户改过）：既不覆盖也不删除。
    Foreign,
}

/// 本模块用到的全部路径（显式传入，便于单测注入）。
struct HookLayout {
    script: PathBuf,
    events: PathBuf,
    backups: PathBuf,
    targets: Vec<HookTarget>,
}

impl HookLayout {
    /// 以 `base` 为「用户主目录」推导全部路径。
    fn under(base: &Path) -> Self {
        let store = base.join(STORE_DIR_NAME);
        Self {
            script: store.join(script_name()),
            events: store.join(EVENTS_FILE_NAME),
            backups: store.join(BACKUP_DIR_NAME),
            targets: TARGETS
                .iter()
                .map(|(dir, label)| {
                    let data_root = base.join(dir);
                    HookTarget {
                        label,
                        settings: data_root.join(SETTINGS_FILE_NAME),
                        data_root,
                    }
                })
                .collect(),
        }
    }

    fn default_layout() -> Self {
        Self::under(&home_dir())
    }

    /// 配置里的 marker：注册命令中脚本路径的**原样形态**（含 [`shell_quote`] 的引号）。
    ///
    /// 必须与写进配置的 command 逐字节一致：否则含单引号 / 空格的路径会让
    /// `contains(marker)` 失配 —— 判定为「未注册」后每次启动都会再追加一条条目。
    /// Windows 的注册命令是 `bash '/c/...'`，marker 同样取 MSYS 形态。
    fn marker(&self) -> String {
        shell_quote(&script_arg(&self.script))
    }

    /// 旧版 `hook.cmd` 注册命令里使用的本工具脚本绝对路径。
    ///
    /// 迁移时只能按这条具体路径识别旧条目，不能用 `.workbuddy-shift` / `hook.cmd`
    /// 等宽泛片段认领第三方命令。
    fn legacy_marker(&self) -> String {
        self.script
            .with_file_name(LEGACY_CMD_NAME)
            .to_string_lossy()
            .to_string()
    }

    /// 上一代数据目录里的 hook 脚本（与当前脚本同名）。
    ///
    /// `self.script` = `<主目录>/<本工具数据目录>/<脚本名>`，因此上跳两层即主目录。
    fn legacy_scripts(&self) -> Vec<PathBuf> {
        let Some(base) = self.script.parent().and_then(Path::parent) else {
            return Vec::new();
        };
        LEGACY_STORE_DIRS
            .iter()
            .map(|dir| base.join(dir).join(script_name()))
            .collect()
    }

    /// 清理用的全部「历史形态」marker：`hook.cmd` 路径 + 上一代目录里的脚本。
    ///
    /// 历史配置里的命令形态不统一（`cmd /c "C:\…\hook.cmd"`、`bash '/c/…/hook.sh'`），
    /// 同一个脚本路径要同时给出原生形态与 MSYS 形态；`contains` 命中任一即认领。
    /// 只用于**清理**，不参与「是否已安装」判定（否则旧条目会挡住自动迁移）。
    fn legacy_markers(&self) -> Vec<String> {
        let mut markers = vec![self.legacy_marker()];
        for script in self.legacy_scripts() {
            markers.push(script.to_string_lossy().to_string());
            markers.push(msys_path(&script));
        }
        markers.sort();
        markers.dedup();
        markers
    }

    /// 是否至少存在一处可接入的客户端（存在的数据根）。
    fn any_target_exists(&self) -> bool {
        self.targets.iter().any(HookTarget::exists)
    }

    /// 已存在且注册成功的客户端数。
    fn registered_target_count(&self) -> usize {
        let marker = self.marker();
        self.targets
            .iter()
            .filter(|target| target.exists() && target.registered(&marker))
            .count()
    }

    /// 读一次脚本文件判断归属（脚本很小，不额外缓存）。
    fn script_ownership(&self) -> ScriptOwnership {
        let Ok(content) = std::fs::read_to_string(&self.script) else {
            return ScriptOwnership::Absent;
        };
        if content == script_body(&self.events) {
            return ScriptOwnership::Current;
        }
        // 旧正文：`$HOME` 形态（最早一版）与「同代模板 + 别的事件路径」（上一代数据目录）。
        // 后者必须认出来 —— 认不出的代价是把本工具自己写的脚本判成「用户改过」，
        // 于是 `write_script` 永久失败、hook 再也不会被注册（2026-11-17 实证）。
        if content == LEGACY_SH_BODY || template_events_target(&content).is_some() {
            return ScriptOwnership::Outdated;
        }
        ScriptOwnership::Foreign
    }

    /// hook 是否已「装全」：脚本不是缺失 / 待升级，且每一处存在的客户端都注册了本工具条目。
    ///
    /// 有客户端存在但一处都没装 / 只装了一半 / 脚本被删 / 脚本还是本工具的旧模板
    /// → 都不算装全（启动时据此重装）。用户改过的脚本按「已接管」处理：
    /// 启动时不去动它，避免反复重装与静默覆盖（升级 / 卸载同样只认本工具生成的正文）。
    fn fully_installed(&self) -> bool {
        !matches!(
            self.script_ownership(),
            ScriptOwnership::Absent | ScriptOwnership::Outdated
        ) && self.any_target_exists()
            && self
                .targets
                .iter()
                .filter(|target| target.exists())
                .all(|target| target.registered(&self.marker()))
    }

    fn backup_path(&self, target: &HookTarget) -> PathBuf {
        self.backups.join(format!("{}.settings.json", target.label))
    }

    /// 「安装时该配置不存在」的标记文件。
    fn absent_mark_path(&self, target: &HookTarget) -> PathBuf {
        self.backups
            .join(format!("{}.settings.json.absent", target.label))
    }
}

mod install;
mod status;
mod uninstall;

pub use install::{auto_install_on_startup, install_hook};
pub use status::{events_path, hook_status};
pub(crate) use status::{hook_marker, needs_log_scan, settings_has_marker};
pub use uninstall::uninstall_hook;

#[cfg(test)]
mod tests {
    use super::*;

    use super::install::*;
    use super::status::*;
    use super::uninstall::*;
    include!("tests/install.rs");
    include!("tests/migration.rs");
    include!("tests/uninstall.rs");
}

//! WorkBuddy / CodeBuddy IDE hook 的轻量 shell 入口（issue #20 回归修复）。
//!
//! ## 为什么不再注册 exe
//!
//! 宿主的 hook 预算按**墙钟**算，包含 `bash` 启动、exe 冷启动、HTTP IPC 排队。
//! v0.4.2 把「等 IPC」改成了即发即走，但每次 hook 仍要冷启动一个 5MB 的
//! `companion-runtime-v1.exe`：慢盘 / 杀软实时扫描 / 冷缓存下实测中位数 1.88–2.19s，
//! 于是把注册超时从 3s 收紧到 1s 之后几乎每次调用都被判超时。
//!
//! 同仓库的积分签到 hook（`wb-shift-core` 的 `rate_limit_hook`）注册的是几 KB 的
//! shell 脚本，只做 `cat` + `>>` + 输出 `{}`，在同样的机器上从未超时 —— 这是本项目里
//! 已经验证过的范式。本模块照此实现，但**不解析 JSON、不拼 JSON**：
//!
//! 1. 脚本按 `(source, edition)` 各生成一份，来源与版本以字面量写死在正文里，
//!    因此注册命令没有任何可变参数（`bash '<脚本>'`），也就不存在参数篡改 / 注入；
//! 2. 每条事件先追加一行来源指令（`hook_frame::SOURCE_DIRECTIVE`），再原样追加
//!    stdin payload 的字节。JSON 拼装全部留给 daemon 侧的 [`crate::hook_frame`]，
//!    脚本里没有一处引号 / 转义拼接；
//! 3. spool 路径安装时解析成**绝对路径**并写死，不依赖 `HOME` / `USERPROFILE`
//!    （hook 由宿主自己的执行器拉起，环境变量不可控），并用 POSIX 单引号转义，
//!    含空格 / 中文 / 单引号的用户目录都不会破坏引号配对；
//! 4. 幂等：正文一致就不重写（不动 mtime），目录与 spool 父目录缺了就补建。
//!
//! codex 不在此列：它的结束事件需要宿主等待采集器确认持久化，仍走 exe + 同步 RPC。

use std::path::{Path, PathBuf};

/// 走脚本的来源（与 `hook_spool::HOST_SOURCES` 一致）。
pub const SCRIPT_SOURCES: [&str; 2] = ["workbuddy", "codebuddy-ide"];

/// 每个来源按客户端版本各一份脚本：版本决定事件上的 `agent_edition`。
pub const SCRIPT_EDITIONS: [&str; 2] = ["domestic", "international"];

/// 脚本所在目录（相对用户主目录）。
const SCRIPT_DIR: &str = ".workbuddy-shift-companion/hooks";

/// 本工具为某个来源 / 版本生成的脚本路径。
pub fn script_path(home: &Path, source: &str, edition: &str) -> PathBuf {
    home.join(SCRIPT_DIR)
        .join(format!("hook-{source}-{edition}.sh"))
}

/// 脚本目录（卸载后若为空一并删除）。
pub fn scripts_dir(home: &Path) -> PathBuf {
    home.join(SCRIPT_DIR)
}

/// 脚本正文。
///
/// 收件箱路径在安装时**写死为绝对路径**（正斜杠形态：MSYS bash 对 `C:/...` 原生
/// 支持，反斜杠在引号里是转义雷区），来源与版本是编译期字面量。
///
/// **一事件一文件**：先把整帧写进自己的私有 `.part`（写多少次都没人跟它抢字节），
/// 再用一次 `mv` 原子改名成 `.evt` 发布。这样并发 hook 之间物理上不可能交错，
/// 也不需要任何锁 —— `mkdir` 自旋锁会在 hook 被强杀时留下死锁，正是 #20 的症状。
/// 详见 [`crate::hook_inbox`]。
pub fn script_body(home: &Path, source: &str, edition: &str) -> String {
    let inbox = shell_quote(&forward_slashes(&crate::hook_inbox::inbox_dir(home)));
    let directive = shell_quote(crate::hook_frame::SOURCE_DIRECTIVE);
    [
        "#!/bin/sh".to_string(),
        "payload=$(cat)".to_string(),
        "if [ -n \"$payload\" ]; then".to_string(),
        format!("  inbox={inbox}"),
        "  file=\"$inbox/$$\"".to_string(),
        // 同一 pid 不会同时出现在两个 hook 里，理论上不会撞名；这里只兜住
        // 「pid 被复用 + 上一条事件还没被消费」的残留情况。
        "  n=0".to_string(),
        "  while [ -e \"$file.evt\" ] || [ -e \"$file.part\" ]; do".to_string(),
        "    n=$((n+1))".to_string(),
        "    file=\"$inbox/$$-$n\"".to_string(),
        "    [ \"$n\" -ge 20 ] && break".to_string(),
        "  done".to_string(),
        format!(
            "  frame() {{ printf '%s %s %s\\n%s\\n' {directive} '{}' '{}' \"$payload\"; }}",
            literal(source),
            literal(edition)
        ),
        // 目录被用户清掉（或首次安装时还没建）会写失败：补建一次再试。
        "  if ! { frame > \"$file.part\"; } 2>/dev/null; then".to_string(),
        "    mkdir -p \"$inbox\" 2>/dev/null".to_string(),
        "    frame > \"$file.part\" 2>/dev/null || rm -f \"$file.part\"".to_string(),
        "  fi".to_string(),
        "  if [ -f \"$file.part\" ]; then".to_string(),
        "    mv -f \"$file.part\" \"$file.evt\" 2>/dev/null || rm -f \"$file.part\"".to_string(),
        "  fi".to_string(),
        "fi".to_string(),
        // 空 stdout 会被宿主当作无效 JSON；`{}` 是三类宿主都接受的空响应。
        "printf '{}'".to_string(),
        String::new(),
    ]
    .join("\n")
}

/// 注册进宿主配置的启动命令：Windows 走 PortableGit 的 `bash`，其余平台走 `sh`。
///
/// 与 `rate_limit_hook` 的选择一致：`cmd /c "..."` 在宿主执行器里会静默失败，
/// 而 `bash "<路径>"` 形态的插件 hook 一直正常。**故意不在安装时探测**
/// `bash` 是否存在：安装进程的 PATH 与宿主执行 hook 时的环境不同（宿主的执行器
/// 本身就是 PortableGit bash），探测失败反而会把慢的 exe 注册回去。
pub fn command(script: &Path) -> String {
    let interpreter = if cfg!(windows) { "bash" } else { "sh" };
    format!("{interpreter} {}", shell_quote(&script_arg(script)))
}

/// 写入 / 升级脚本（幂等）。
///
/// 同时补建脚本目录与**收件箱目录**：两者缺失都会让事件静默丢失（重定向失败只写
/// 脚本自己的 stderr，宿主看不到），所以每次安装都确认一次，代价只是几次 stat。
pub fn ensure(home: &Path, source: &str, edition: &str) -> Result<PathBuf, String> {
    let script = script_path(home, source, edition);
    for dir in [scripts_dir(home), crate::hook_inbox::inbox_dir(home)] {
        std::fs::create_dir_all(&dir).map_err(|e| format!("创建 {} 失败：{e}", dir.display()))?;
    }
    let body = script_body(home, source, edition);
    if std::fs::read_to_string(&script).is_ok_and(|current| current == body) {
        return Ok(script);
    }
    std::fs::write(&script, body).map_err(|e| format!("写入 {} 失败：{e}", script.display()))?;
    Ok(script)
}

/// 删除某个来源的两份脚本（卸载该来源时调用：注册项已摘掉，脚本不该再留着）。
pub fn remove_source(home: &Path, source: &str) -> Vec<String> {
    SCRIPT_EDITIONS
        .iter()
        .filter_map(|edition| remove(&script_path(home, source, edition)))
        .collect()
}

/// 删除全部生成的脚本与（空了的）脚本目录。
pub fn remove_all(home: &Path) -> Vec<String> {
    let mut errors = Vec::new();
    for source in SCRIPT_SOURCES {
        errors.extend(remove_source(home, source));
    }
    // 目录里可能还有别的文件：只在空目录时删除，绝不递归清理用户目录。
    let dir = scripts_dir(home);
    if dir.is_dir() {
        let empty = std::fs::read_dir(&dir).is_ok_and(|mut entries| entries.next().is_none());
        if empty {
            if let Err(e) = std::fs::remove_dir(&dir) {
                errors.push(format!("删除 {} 失败：{e}", dir.display()));
            }
        }
    }
    errors
}

/// 命令是否是本工具为 `source` 生成的脚本命令（逐字节，覆盖另一种 edition 的残留）。
pub fn is_script_command_for(home: &Path, source: &str, candidate: &str) -> bool {
    SCRIPT_EDITIONS
        .iter()
        .any(|edition| command(&script_path(home, source, edition)) == candidate)
}

/// 命令是否是「用 sh / bash 跑本工具某一个脚本」的形态（容忍引号 / 斜杠写法差异）。
///
/// 只接受**整条命令就是 `<解释器> <脚本>`**：`echo '<脚本>'` 这类第三方包装命令
/// 不认领（否则会把用户自己的 wrapper 删掉），`bash '<…>/.workbuddy-shift/hook.sh'`
/// 这类积分签到命令也不认领（路径对不上本工具脚本）。
///
/// 用途是兜住「逐字节比对失配」的等价写法（例如少一层引号），否则同一条命令会被
/// 判成第三方而保留下来 ⇒ 同一事件触发两次。
pub fn is_script_invocation(home: &Path, candidate: &str) -> bool {
    let mut parts = candidate.split_whitespace();
    let interpreter = parts.next().unwrap_or_default();
    if !matches!(interpreter, "bash" | "sh") {
        return false;
    }
    let Some(argument) = parts.next() else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }
    let target = normalize_path_text(argument);
    SCRIPT_SOURCES.iter().any(|source| {
        SCRIPT_EDITIONS.iter().any(|edition| {
            let script = script_path(home, source, edition);
            let native = normalize_path_text(&forward_slashes(&script));
            let msys = normalize_path_text(&msys_path(&script));
            target == native || target == msys
        })
    })
}

/// 去掉一层引号并把反斜杠统一成正斜杠，用于等价写法比较（不做子串匹配）。
fn normalize_path_text(value: &str) -> String {
    value
        .trim_matches(|c| c == '\'' || c == '"')
        .replace('\\', "/")
}

fn remove(script: &Path) -> Option<String> {
    match std::fs::remove_file(script) {
        Ok(()) => None,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => Some(format!("删除 {} 失败：{e}", script.display())),
    }
}

/// 正斜杠形态：脚本里写死的路径（MSYS bash 原生支持 `C:/...`）。
fn forward_slashes(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// MSYS 风格路径：`C:\a\b` → `/c/a/b`（PortableGit bash 的原生形态）。
fn msys_path(script: &Path) -> String {
    let text = forward_slashes(script);
    let mut chars = text.chars();
    let drive = chars.next().unwrap_or('c').to_ascii_lowercase();
    match chars.next() {
        Some(':') => format!("/{}{}", drive, chars.as_str()),
        _ => text,
    }
}

/// 注册命令里脚本路径的**实参形态**：Windows 走 MSYS 路径，其余平台原样。
fn script_arg(script: &Path) -> String {
    if cfg!(windows) {
        msys_path(script)
    } else {
        script.to_string_lossy().to_string()
    }
}

/// 单引号包裹 shell 参数（路径可能含空格 / 中文 / 单引号）。
///
/// 单引号按 POSIX 写法转义为 `'\''`：这是唯一的引号形态，写进配置的 command
/// 与「是否已注册」的 marker 必须逐字节一致。
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// 正文里的字面量：来源 / 版本来自编译期常量，这里只做防御性校验。
fn literal(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}

#[cfg(test)]
mod tests;

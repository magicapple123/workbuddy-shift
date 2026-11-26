use super::{ensure_hook_binary, hook_command};
use companion_core::{
    adapters::{
        codebuddy_edition, codebuddy_settings_files, collect_session_logs, is_session_log,
        is_workbuddy_hook_command, legacy_hook_command, workbuddy_edition,
        workbuddy_settings_files, Collector, CODEBUDDY_IDE_HOOK_EVENTS, WORKBUDDY_HOOK_EVENTS,
    },
    atomic_json,
    settings::SOURCES,
    text,
};
use companion_runtime::hook_script;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

// 「批量卸载」单独成文件：主文件已接近 AGENTS.md 第 4 节的 500 行阈值，而它与
// 「单个来源的接入状态 / 安装」是两份独立职责。对外路径不变
// （`crate::integrations::uninstall_all_hooks`），只是实现换了个文件。
mod uninstall;
pub use uninstall::uninstall_all_hooks;

const CODEX_EVENTS: [&str; 8] = [
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PermissionRequest",
    "Stop",
    "Interrupt",
    "SessionEnd",
];
/// 接入的三种类型：装 hook 的、注册 webhook 的、以及**只读日志**的。
///
/// DSH 没有可装的 hook（它的 hook 覆盖面窄且要改用户 `profiles` 配置），
/// 所以它的接入状态完全由「会话目录能不能读」决定。
fn kind_of(source: &str) -> &'static str {
    match source {
        "codeg" => "webhook",
        "dsh" => "log",
        _ => "hooks",
    }
}
fn events(source: &str) -> &[&str] {
    match source {
        "codex" => &CODEX_EVENTS,
        "workbuddy" => &WORKBUDDY_HOOK_EVENTS,
        // 只读日志来源没有 hook 事件：返回空避免误入 hook 合并逻辑。
        "dsh" => &[],
        _ => &CODEBUDDY_IDE_HOOK_EVENTS,
    }
}
fn files(c: &Collector, source: &str) -> Vec<PathBuf> {
    let custom = text(&c.settings["sources"][source]["path"]);
    match source {
        "codex" => vec![c.paths(source)[0].join("hooks.json")],
        "workbuddy" => workbuddy_settings_files(&c.home, &custom),
        "dsh" => Vec::new(),
        _ => codebuddy_settings_files(&c.home, &custom),
    }
}
/// DSH 让用户去打开的就是会话日志目录。
fn dsh_locations(c: &Collector) -> Vec<PathBuf> {
    let dir = c.dsh_sessions_dir();
    if dir.is_dir() {
        vec![dir]
    } else {
        Vec::new()
    }
}
/// DSH 接入状态：目录可读即「已连接（只读）」，读不到就「暂不可用」。
///
/// 这里绝不写盘——`$DSH_HOME` 下的任何文件都不碰，也不装 hook。
fn dsh_status(c: &Collector) -> (&'static str, String) {
    match std::fs::read_dir(c.dsh_sessions_dir()) {
        Err(_) => (
            "unavailable",
            "暂不可用：未找到 DSH 会话目录（$DSH_HOME/sessions）".into(),
        ),
        Ok(_) => match dsh_log_count(c) {
            0 => ("installed", "已连接（只读）；暂无会话记录".into()),
            count => (
                "installed",
                format!("已连接（只读）；已发现 {count} 个会话日志"),
            ),
        },
    }
}
/// 数一下现有会话日志。上千个会话时截断，接入页不需要精确值。
fn dsh_log_count(c: &Collector) -> usize {
    const CAP: usize = 500;
    let mut logs = Vec::new();
    collect_session_logs(&c.dsh_sessions_dir(), &mut logs);
    logs.iter()
        .filter(|path| is_session_log(path))
        .take(CAP)
        .count()
}
fn binary(c: &Collector) -> PathBuf {
    c.home.join(if cfg!(windows) {
        ".workbuddy-shift-companion/bin/companion-runtime-v1.exe"
    } else {
        ".workbuddy-shift-companion/bin/companion-runtime-v1"
    })
}

/// 该来源在这个配置文件里对应哪个客户端版本（决定脚本与 `agent_edition`）。
///
/// codex 没有版本维度：它仍走 exe + 同步 RPC，返回 `None`。
fn edition_of(source: &str, file: &Path) -> Option<&'static str> {
    match source {
        "workbuddy" => Some(workbuddy_edition(file)),
        "codebuddy-ide" => Some(codebuddy_edition(file)),
        _ => None,
    }
}

/// 写进宿主配置的注册命令。
///
/// WorkBuddy / CodeBuddy IDE：`bash '<脚本>'`。宿主的 hook 预算包含进程冷启动，
/// 注册 5MB 的 exe 在慢盘 / 杀软环境下要 1.88–2.19s（issue #20 回归），而脚本
/// 只做 `cat` + `>>` + 打印 `{}`。codex 的结束事件需要同步确认持久化，保留 exe。
fn command(c: &Collector, source: &str, file: &Path) -> String {
    match edition_of(source, file) {
        Some(edition) => hook_script::command(&hook_script::script_path(&c.home, source, edition)),
        None => hook_command(&binary(c), &c.home, source, None),
    }
}

/// 注册项指向的运行文件是否就位（脚本或 exe），用于接入状态判定。
fn hook_ready(c: &Collector, source: &str, file: &Path) -> bool {
    match edition_of(source, file) {
        Some(edition) => hook_script::script_path(&c.home, source, edition).is_file(),
        None => binary(c).is_file(),
    }
}

/// 从 hook 命令里取 `--source` 参数（兼容单双引号）。
fn hook_source_argument(command: &str) -> Option<&str> {
    let mut tokens = command.split_whitespace();
    while let Some(token) = tokens.next() {
        if token == "--source" {
            return tokens
                .next()
                .map(|value| value.trim_matches(|c| c == '\'' || c == '"'));
        }
    }
    None
}

/// 红线筛查：只有看起来属于 companion / 旧 agent-studio 的命令才进入删除判定；
/// `bash '<…>/.workbuddy-shift/hook.sh'`（积分签到）、hook-backups、第三方 hook
/// 永远不参与。
///
/// 三种需要认领的形态：当前的 `bash '<脚本>'`、v0.4.2 的 exe 命令
/// （`companion-runtime … hook … --source …`）、旧 `.agent-studio` 命令。
fn may_be_ours(home: &Path, command: &str) -> bool {
    is_workbuddy_hook_command(command)
        || legacy_hook_command(command)
        || command.contains("companion-runtime")
        || hook_script::is_script_invocation(home, command)
}

/// Match complete commands at known application-owned locations, never substring names.
///
/// 除了当前命令，还识别：同品牌无/旧 `--edition` 变体、同来源另一种版本的脚本、
/// 以及旧 `.agent-studio` 品牌指向同一来源的入口。旧条目必须在这里摘掉，否则一个事件
/// 会被新旧两个入口各触发一次（issue #20 的「11 事件 × 2 程序」）。
fn owned(c: &Collector, source: &str, file: &Path, value: &Value) -> bool {
    let cmd = text(&value["command"]);
    let cmd = cmd.trim();
    if !may_be_ours(&c.home, cmd) {
        return false;
    }
    if cmd == command(c, source, file) {
        return true;
    }
    // 同一来源另一种版本的脚本命令（用户改过配置路径后会残留）。
    if hook_script::is_script_command_for(&c.home, source, cmd) {
        return true;
    }
    // v0.4.2 的 exe 命令：有无 `--edition` 的所有变体都以同一个前缀开头。
    let prefix = hook_command(&binary(c), &c.home, source, None);
    if cmd == prefix || cmd.starts_with(&prefix) {
        return true;
    }
    // 兜住同一个脚本的另一种书写（例如用户 / 编辑器少一层引号）：只要指向本工具的
    // 脚本目录就认领。逐字节比对是主判据，这一层只防「判成第三方 ⇒ 双份注册 ⇒
    // 同一事件触发两次」。认领范围仍限定在本工具的专属目录内。
    if hook_script::is_script_invocation(&c.home, cmd) {
        return true;
    }
    legacy_hook_command(cmd) && hook_source_argument(cmd) == Some(source)
}

/// 旧品牌独立配置目录里遗留的注册项（升级前的 settings.json）。
///
/// 三种形态都认：旧 `.agent-studio` 入口、v0.4.2 的 exe 命令、新的脚本命令。
fn legacy_owned(home: &Path, value: &Value) -> bool {
    let cmd = text(&value["command"]);
    let cmd = cmd.trim();
    legacy_hook_command(cmd)
        || cmd.contains("companion-runtime")
        || hook_script::is_script_invocation(home, cmd)
}
fn read(file: &Path) -> Result<Value, String> {
    let v: Value = match std::fs::read(file) {
        Ok(b) => serde_json::from_slice(&b)
            .map_err(|_| format!("{} 配置不是有效 JSON，未修改", file.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(e) => return Err(format!("{}: {e}", file.display())),
    };
    if !v.is_object() || (!v["hooks"].is_null() && !v["hooks"].is_object()) {
        return Err(format!("{} Hooks 配置无效，未修改", file.display()));
    }
    if let Some(map) = v["hooks"].as_object() {
        for groups in map.values() {
            let groups = groups.as_array().ok_or("Hook 事件配置无效，未修改")?;
            for group in groups {
                if !group.is_object()
                    || (group.get("hooks").is_some() && !group["hooks"].is_array())
                {
                    return Err("Hook 分组配置无效，未修改".into());
                }
            }
        }
    }
    Ok(v)
}
/// 从 hooks 配置里摘掉匹配 `is_owned` 的分组/条目，保留其它一切字段与顺序。
fn strip_owned(doc: &mut Value, is_owned: &impl Fn(&Value) -> bool) {
    if let Some(map) = doc["hooks"].as_object_mut() {
        for groups in map.values_mut() {
            let Some(groups) = groups.as_array_mut() else {
                continue;
            };
            groups.retain_mut(|group| {
                if is_owned(group) {
                    return false;
                }
                if let Some(hooks) = group["hooks"].as_array_mut() {
                    let count = hooks.len();
                    hooks.retain(|hook| !is_owned(hook));
                    return count == hooks.len() || !hooks.is_empty();
                }
                true
            });
        }
    }
}

fn contains(c: &Collector, source: &str, file: &Path, groups: &Value) -> bool {
    groups.as_array().into_iter().flatten().any(|g| {
        owned(c, source, file, g)
            || g["hooks"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|h| owned(c, source, file, h))
    })
}
fn edit(c: &Collector, source: &str, install: bool) -> Result<(), String> {
    let targets = files(c, source);
    if install
        && (targets.is_empty()
            || targets
                .iter()
                .any(|p| !p.parent().is_some_and(Path::is_dir)))
    {
        return Err("未找到应用配置目录，请先安装应用或设置路径".into());
    }
    // Validate every target before writing either edition.
    let documents: Vec<_> = targets
        .iter()
        .map(|p| read(p).map(|v| (p, v)))
        .collect::<Result<_, _>>()?;
    if install {
        // 脚本 / exe 都必须先落地：注册的命令指向它们。
        for file in &targets {
            match edition_of(source, file) {
                Some(edition) => {
                    hook_script::ensure(&c.home, source, edition)?;
                }
                None => {
                    ensure_hook_binary(&c.home)?;
                }
            }
        }
    }
    // 配置写盘失败不再立刻 `?` 返回：那会让「另一个文件的写盘失败」看起来像「什么都没做」。
    // 逐个收集，最后与脚本删除的错误一起汇总，并据此决定要不要删脚本。
    let mut errors: Vec<String> = Vec::new();
    for (file, before) in documents {
        let mut doc = before.clone();
        if install && doc["hooks"].is_null() {
            doc["hooks"] = json!({});
        }
        strip_owned(&mut doc, &|value| owned(c, source, file, value));
        if install {
            for event in events(source) {
                if doc["hooks"][event].is_null() {
                    doc["hooks"][event] = json!([]);
                }
                // 宿主给 hook 的预算是 3000ms（包含进程启动）。脚本形态实测在几十
                // 毫秒级，3 秒是充足兜底；**不得**再收紧 —— 1 秒会让慢盘机器每次
                // 调用都被判超时（issue #20 的回归）。
                let timeout = 3;
                let mut h =
                    json!({"type":"command","command":command(c,source,file),"timeout":timeout});
                if source != "codebuddy-ide" {
                    h["statusMessage"] = json!("WorkBuddy Shift");
                    // Codex CLI skips handlers marked async entirely. Keep all
                    // Codex hooks synchronous so their lifecycle is observable.
                    if source == "workbuddy" && *event != "SessionEnd" {
                        h["async"] = json!(true);
                    }
                }
                let group = if source == "codex" {
                    json!({"hooks":[h]})
                } else {
                    json!({"matcher":"","hooks":[h]})
                };
                doc["hooks"][event].as_array_mut().unwrap().push(group);
            }
        }
        if doc != before {
            let backup = file.parent().unwrap().join(match source {
                "codex" => "hooks.companion-before-rust.json",
                "workbuddy" => "settings.companion-before-hooks.json",
                _ => "settings.companion-before-ide-hooks.json",
            });
            if file.exists() && !backup.exists() {
                if let Err(e) = atomic_json(&backup, &before) {
                    // 没有「改前快照」就不动这份配置：宁可这一份没改成，
                    // 也不能留下无法回滚的修改。
                    errors.push(e);
                    continue;
                }
            }
            if let Err(e) = atomic_json(file, &doc) {
                errors.push(e);
            }
        }
    }
    if !install && hook_script::SCRIPT_SOURCES.contains(&source) {
        // 注册项已摘掉，脚本文件不该再留着；spool 与其中未消费的事件一律保留
        // （下次启动 / 重装仍要补采）。
        //
        // 这里**不再立刻 `return Err`**：`documents` 里的配置此时已经落盘（注册项
        // 已清），立刻返回会让调用方把「脚本删不掉」读成「整个卸载被回滚了」。
        // 收集起来与配置写盘失败一起汇总，语义才是可读的
        // 「注册项已清，脚本可能残留」。`uninstall_all_hooks` 末尾还会再兜底
        // 调一次 `hook_script::remove_all`，所以残留脚本仍有第二次删除机会。
        if errors.is_empty() {
            errors.extend(hook_script::remove_source(&c.home, source));
        }
        // 配置没清干净时故意保留脚本：残留的注册项还指向它，删掉会让宿主拿到
        // 空 stdout（WorkBuddy 会把空输出当作无效 JSON）。此时 errors 非空且会被返回。
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("；"))
    }
}
pub fn install_hooks(c: &Collector) -> Result<(), String> {
    let mut errors = vec![];
    for source in ["codex", "workbuddy", "codebuddy-ide"] {
        if c.settings["sources"][source]["enabled"] == true && c.integration_automatic(source) {
            // First launch before the agent itself is installed remains harmless.
            if files(c, source)
                .iter()
                .any(|p| p.parent().is_some_and(Path::is_dir))
            {
                if let Err(e) = edit(c, source, true) {
                    errors.push(e);
                }
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("；"))
    }
}

pub fn get(c: &Collector) -> Value {
    let sources: Vec<_> = SOURCES.iter().map(|source| {
        let locations = match kind_of(source) {
            "webhook" => c.paths(source),
            "log" => dsh_locations(c),
            _ => files(c,source),
        };
        let result = match kind_of(source) {
            "webhook" => c.inspect_codeg_webhook(),
            "log" => Ok(dsh_status(c)),
            _ => (|| {
                if locations.is_empty() || locations.iter().all(|p| !p.parent().is_some_and(Path::is_dir)) { return Ok(("unavailable", "未找到应用配置目录".into())); }
                let mut total = 0; let mut present = 0; let mut any_owned = false; let mut ready = true;
                for file in &locations {
                    // 注册项指向的运行文件（脚本或 exe）必须就位，否则 hook 一定失败。
                    ready &= hook_ready(c, source, file);
                    let doc = read(file)?;
                    any_owned |= doc["hooks"].as_object().is_some_and(|m|m.values().any(|g|contains(c,source,file,g)));
                    for event in events(source) { total += 1; if contains(c,source,file,&doc["hooks"][event]) { present += 1; } }
                }
                if present == total && total > 0 && ready { Ok(("installed", "Hooks 已安装".into())) }
                else if any_owned { Ok(("partial", "Hooks 不完整或运行文件缺失，请修复".into())) }
                else { Ok(("not_installed", "Hooks 未安装".into())) }
            })(),
        };
        let (status,message) = result.unwrap_or_else(|e|("error",e));
        json!({"source":source,"kind":kind_of(source),"status":status,"message":message,"locations":locations,"automatic":c.integration_automatic(source),"lastEventAt":c.last_hook_at.get(*source)})
    }).collect();
    json!({"sources":sources})
}
pub fn set(c: &mut Collector, p: &Value) -> Result<Value, String> {
    let source = p["source"]
        .as_str()
        .filter(|s| SOURCES.contains(s))
        .ok_or("未知接入来源")?;
    let install = match p["action"].as_str() {
        Some("install") => true,
        Some("uninstall") => false,
        _ => return Err("未知接入操作".into()),
    };
    if source == "codeg" && install && c.settings["sources"][source]["enabled"] != true {
        return Err("请先开启 Codeg 监听，再注册 Webhook".into());
    }
    if kind_of(source) == "log" {
        // 只读来源：install / uninstall 只切「自动接入」开关，不写任何文件。
        // DSH 的会话日志与配置全在 $DSH_HOME，我们一个字节都不碰。
        c.set_integration_automatic(source, install)?;
        if !install {
            c.reset_dsh();
        }
        return Ok(get(c));
    }
    c.set_integration_automatic(source, install)?;
    if source == "codeg" {
        c.codeg.registered = false;
        c.codeg.next_attempt = None;
        // Preserve pending ownership on offline cleanup; the status shows pending.
        if let Err(e) = c.maintain_codeg_webhook() {
            if install {
                return Err(e);
            }
        }
    } else {
        edit(c, source, install)?;
    }
    Ok(get(c))
}

#[cfg(test)]
mod qa_tests;
#[cfg(test)]
mod tests;

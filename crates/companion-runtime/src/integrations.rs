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
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
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
fn command(c: &Collector, source: &str, file: &Path) -> String {
    let edition = match source {
        "workbuddy" => Some(workbuddy_edition(file)),
        "codebuddy-ide" => Some(codebuddy_edition(file)),
        _ => None,
    };
    hook_command(&binary(c), &c.home, source, edition)
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
/// `bash hook.sh`（积分签到）、hook-backups 等永远不参与。
fn may_be_ours(command: &str) -> bool {
    is_workbuddy_hook_command(command)
        || legacy_hook_command(command)
        || command.contains("companion-runtime")
}

/// Match complete commands at known application-owned locations, never substring names.
///
/// 除了当前命令，还识别：同品牌无/旧 `--edition` 变体，以及旧 `.agent-studio`
/// 品牌指向同一来源的入口。旧条目必须在这里摘掉，否则一个事件会被新旧两个 exe
/// 各触发一次（issue #20 的「11 事件 × 2 程序」）。
fn owned(c: &Collector, source: &str, file: &Path, value: &Value) -> bool {
    let cmd = text(&value["command"]);
    let cmd = cmd.trim();
    if !may_be_ours(cmd) {
        return false;
    }
    if cmd == command(c, source, file) || cmd == hook_command(&binary(c), &c.home, source, None) {
        return true;
    }
    // 复用真正的注册命令构造器：任何 --edition 变体都以此开头，且与写入端共用同一套引号规则。
    let prefix = hook_command(&binary(c), &c.home, source, None);
    if cmd.starts_with(&prefix) {
        return true;
    }
    legacy_hook_command(cmd) && hook_source_argument(cmd) == Some(source)
}

/// 旧品牌独立配置目录里遗留的注册项（升级前的 settings.json）。
fn legacy_owned(value: &Value) -> bool {
    legacy_hook_command(&text(&value["command"]))
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
        ensure_hook_binary(&c.home)?;
    }
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
                // WorkBuddy / IDE 的 hook 不再发 RPC，1 秒只是宿主侧兜底；codex
                // 的结束事件仍需等待采集器确认持久化，保留原来的 3 秒。
                let timeout = if source == "codex" { 3 } else { 1 };
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
                atomic_json(&backup, &before)?;
            }
            atomic_json(file, &doc)?;
        }
    }
    Ok(())
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
/// 批量移除全部已注册的 companion hook，并写入「不再自动接入」策略。
///
/// 供 `companion-runtime uninstall-hooks --home <dir>`（安装器卸载宏 / 用户手动）
/// 调用。只删除本工具当前品牌与旧 `.agent-studio` 品牌的注册项；积分签到的
/// `hook.sh`、第三方 hook、备份文件与其它数据一律保留。
pub fn uninstall_all_hooks(c: &mut Collector) -> Result<(), String> {
    let mut errors = Vec::new();
    for source in ["codex", "workbuddy", "codebuddy-ide"] {
        if let Err(e) = edit(c, source, false) {
            errors.push(e);
        }
        // 防止下次 serve 启动 / settings_set 时静默重装（issue #20 的「关掉还在拦」）。
        if let Err(e) = c.set_integration_automatic(source, false) {
            errors.push(e);
        }
    }
    // 旧品牌自己的配置目录也可能留有注册项；它不在 files() 的扫描范围内。
    let legacy_file = c.home.join(".agent-studio/settings.json");
    if legacy_file.exists() {
        match read(&legacy_file) {
            Ok(before) => {
                let mut doc = before.clone();
                strip_owned(&mut doc, &legacy_owned);
                if doc != before {
                    if let Err(e) = atomic_json(&legacy_file, &doc) {
                        errors.push(e);
                    }
                }
            }
            Err(e) => errors.push(e),
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
                let mut total = 0; let mut present = 0; let mut any_owned = false;
                for file in &locations {
                    let doc = read(file)?;
                    any_owned |= doc["hooks"].as_object().is_some_and(|m|m.values().any(|g|contains(c,source,file,g)));
                    for event in events(source) { total += 1; if contains(c,source,file,&doc["hooks"][event]) { present += 1; } }
                }
                if present == total && total > 0 && binary(c).is_file() { Ok(("installed", "Hooks 已安装".into())) }
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
mod tests;

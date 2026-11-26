//! `hook_script` 的用例：脚本正文、注册命令形态、所有权判定、真跑与并发发布。
//!
//! 从 `hook_script.rs` 里拆出来：正文部分加上用例会越过 `AGENTS.md` 第 4 节的 500 行
//! 阈值，而测试与脚本生成是两份独立职责（公开 API 未变，只是用例位置变化）。

use super::*;
use std::io::Write;

fn temp_home(tag: &str) -> PathBuf {
    let home = std::env::temp_dir().join(format!("{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&home).unwrap();
    home
}

#[test]
fn script_body_carries_source_edition_and_an_absolute_inbox_path() {
    let home = temp_home("hook-script-body");
    let body = script_body(&home, "workbuddy", "domestic");
    assert!(body.starts_with("#!/bin/sh\n"));
    assert!(body.contains("payload=$(cat)"));
    assert!(body.contains("@@workbuddy-shift-hook-source"));
    assert!(body.contains("'workbuddy' 'domestic'"));
    assert!(
        body.contains(&forward_slashes(&crate::hook_inbox::inbox_dir(&home))),
        "收件箱路径必须是写死的绝对路径：{body}"
    );
    // 一事件一文件：先写私有 .part 再 mv 发布，绝不能追加到共享文件。
    assert!(body.contains(".part"));
    assert!(body.contains("mv -f"));
    assert!(
        !body.contains(">>"),
        "脚本不得再追加到共享文件（并发会交错）：{body}"
    );
    assert!(body.trim_end().ends_with("printf '{}'"));
    assert!(!body.contains("companion-runtime"), "脚本不得再拉起 exe");
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn script_body_survives_spaces_cjk_and_single_quotes_in_the_home_path() {
    let home = std::env::temp_dir()
        .join("hook script 中文 o'brien")
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&home).unwrap();
    let body = script_body(&home, "codebuddy-ide", "international");
    let quoted = shell_quote(&forward_slashes(&crate::hook_inbox::inbox_dir(&home)));
    assert!(body.contains(&quoted), "{body}");
    // 单引号必须按 POSIX 形态转义，否则整条重定向会在第一个引号处截断。
    assert!(quoted.contains("o'\\''brien"), "{quoted}");
    assert_eq!(shell_quote("o'brien"), "'o'\\''brien'");
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn is_script_invocation_accepts_equivalent_spellings_and_rejects_wrappers() {
    let home = temp_home("hook-script-invocation");
    let script = script_path(&home, "workbuddy", "domestic");
    // 宿主实际执行的形态。
    assert!(is_script_invocation(&home, &command(&script)));
    // 等价写法（少一层引号 / 原生路径 / 双向斜杠）都算我们的。
    let native = script.to_string_lossy().replace('\\', "/");
    assert!(is_script_invocation(&home, &format!("bash {native}")));
    assert!(is_script_invocation(&home, &format!("sh '{native}'")));
    assert!(is_script_invocation(
        &home,
        &format!("bash '{}'", native.replace('/', "\\"))
    ));
    // 第三方包装 / 另一个脚本 / 其它解释器都不认领。
    for foreign in [
        format!("echo '{native}'"),
        format!("bash -x '{native}'"),
        "bash hook.sh".to_string(),
        "bash '/c/Users/example/.workbuddy-shift/hook.sh'".to_string(),
        "node '/c/Users/example/plugins/tracker/index.js'".to_string(),
    ] {
        assert!(
            !is_script_invocation(&home, &foreign),
            "不得认领第三方命令：{foreign}"
        );
    }
    // 别的 edition 的脚本同样属于本工具（改过配置路径后的残留）。
    let international = script_path(&home, "workbuddy", "international");
    assert!(is_script_invocation(&home, &command(&international)));
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn registered_command_uses_the_platform_interpreter_and_quotes_the_path() {
    let home = temp_home("hook-script-command");
    let script = script_path(&home, "workbuddy", "domestic");
    let registered = command(&script);
    let (interpreter, rest) = registered.split_once(' ').unwrap();
    assert_eq!(interpreter, if cfg!(windows) { "bash" } else { "sh" });
    assert!(rest.starts_with('\'') && rest.ends_with('\''));
    assert!(rest.contains("hook-workbuddy-domestic.sh"));
    if cfg!(windows) {
        // 不写死 `/c/`：主目录在 D: 盘的机器是 `/d/…`。只断言 MSYS 形态
        // 「/ + 小写盘符 + /」。
        let inner = rest.trim_matches('\'');
        let mut chars = inner.trim_start_matches('/').chars();
        assert!(
            chars.next().is_some_and(|c| c.is_ascii_lowercase()) && chars.next() == Some('/'),
            "Windows 必须用 MSYS 路径：{rest}"
        );
    }
    // 逐字节稳定：marker 与注册命令共用同一套引号规则。
    assert_eq!(registered, super::command(&script));
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn ensure_is_idempotent_and_prunes_only_its_own_scripts() {
    let home = temp_home("hook-script-ensure");
    let script = ensure(&home, "workbuddy", "domestic").unwrap();
    assert!(script.is_file());
    assert!(
        crate::hook_inbox::inbox_dir(&home).is_dir(),
        "收件箱目录必须随安装一起创建"
    );
    let first = std::fs::read_to_string(&script).unwrap();
    ensure(&home, "workbuddy", "domestic").unwrap();
    assert_eq!(std::fs::read_to_string(&script).unwrap(), first);
    ensure(&home, "workbuddy", "international").unwrap();
    ensure(&home, "codebuddy-ide", "domestic").unwrap();
    // 收件箱里待消费的事件不得被卸载逻辑删掉。
    let pending = crate::hook_inbox::inbox_dir(&home).join("42.evt");
    std::fs::write(&pending, "{}").unwrap();
    // 用户在该目录里放的文件不得被卸载逻辑删掉。
    let foreign = scripts_dir(&home).join("notes.txt");
    std::fs::write(&foreign, "keep").unwrap();
    assert!(remove_all(&home).is_empty());
    assert!(!script.exists());
    assert!(foreign.is_file(), "脚本目录里的第三方文件必须保留");
    assert!(pending.is_file(), "未消费的事件不得随卸载删除");
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn remove_source_leaves_the_other_source_alone() {
    let home = temp_home("hook-script-remove-source");
    let workbuddy = ensure(&home, "workbuddy", "domestic").unwrap();
    let ide = ensure(&home, "codebuddy-ide", "domestic").unwrap();
    assert!(remove_source(&home, "workbuddy").is_empty());
    assert!(!workbuddy.exists());
    assert!(ide.is_file());
    std::fs::remove_dir_all(&home).unwrap();
}

/// 找到一个能跑 sh 脚本的解释器；都没有就跳过（CI 的 Windows runner 没有
/// PortableGit，本机与 macOS / Linux 一定有）。
fn interpreter() -> Option<&'static str> {
    ["bash", "sh"].into_iter().find(|name| {
        std::process::Command::new(name)
            .arg("-c")
            .arg("exit 0")
            .status()
            .is_ok_and(|status| status.success())
    })
}

#[test]
fn the_script_publishes_the_event_without_any_daemon_and_prints_an_empty_object() {
    let Some(interpreter) = interpreter() else {
        eprintln!("跳过：本机没有可用于执行的 sh / bash");
        return;
    };
    let home = temp_home("hook-script-run");
    let script = ensure(&home, "workbuddy", "domestic").unwrap();
    let payload = r#"{"hook_event_name":"Stop","session_id":"o'brien"}"#;
    let mut child = std::process::Command::new(interpreter)
        .arg(script.to_string_lossy().replace('\\', "/"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "脚本执行失败：{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "{}");
    assert!(output.stderr.is_empty(), "{output:?}");
    // 事件必须原样落进收件箱（一事件一文件），且能被队列读成「带来源」的事件。
    let inbox = crate::hook_inbox::inbox_dir(&home);
    let published: Vec<_> = std::fs::read_dir(&inbox)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .collect();
    assert_eq!(
        published.len(),
        1,
        "必须恰好发布一个事件文件：{published:?}"
    );
    let frame = std::fs::read_to_string(&published[0]).unwrap();
    assert!(frame.starts_with("@@workbuddy-shift-hook-source"));
    assert!(frame.contains(payload), "{frame}");
    assert!(
        !crate::hook_spool::spool_path(&home).exists(),
        "脚本不得再写共享 spool 文件"
    );
    let mut reader = crate::HookSpool::new(&home);
    let mut events = Vec::new();
    assert_eq!(reader.drain(|v| events.push(v)).unwrap(), 1);
    assert_eq!(events[0]["agent_source"], "workbuddy");
    assert_eq!(events[0]["agent_edition"], "domestic");
    assert_eq!(events[0]["session_id"], "o'brien");
    assert_eq!(
        std::fs::read_dir(&inbox).unwrap().count(),
        0,
        "消费后事件文件必须删除"
    );
    std::fs::remove_dir_all(&home).unwrap();
}

/// P4-1 回归：收件箱目录不存在时，**失败的重定向本身**不能把错误漏到宿主 stderr。
///
/// `if ! frame > "$file.part" 2>/dev/null; then` 的问题在于 bash 从左到右应用重定向：
/// `> "$file.part"` 先失败，`2>/dev/null` 还没生效，`No such file or directory`
/// 就写进了继承来的 stderr。`{ …; } 2>/dev/null` 把 fd 2 先换掉，失败消息才被吃掉。
/// 语义必须不变：仍 exit 0、stdout 恰好 `{}`、补建目录后事件照样落地。
#[test]
fn a_missing_inbox_directory_leaves_stderr_empty_and_still_publishes() {
    let Some(interpreter) = interpreter() else {
        eprintln!("跳过：本机没有可用于执行的 sh / bash");
        return;
    };
    let home = temp_home("hook-script-missing-inbox");
    let script = ensure(&home, "workbuddy", "domestic").unwrap();
    let inbox = crate::hook_inbox::inbox_dir(&home);
    // 用户把收件箱目录清掉（首次安装还没建时也是同一形态）。
    std::fs::remove_dir_all(&inbox).unwrap();
    let payload = r#"{"session_id":"no-inbox","hook_event_name":"Stop"}"#;
    let mut child = std::process::Command::new(interpreter)
        .arg(script.to_string_lossy().replace('\\', "/"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "脚本必须仍然 exit 0：{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "{}");
    assert!(
        output.stderr.is_empty(),
        "目录缺失时 stderr 必须为空（宿主会把它当 hook 故障）：{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // 补建目录后事件必须照样发布，一条不多一条不少。
    let published: Vec<_> = std::fs::read_dir(&inbox)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .collect();
    assert_eq!(published.len(), 1, "事件必须仍落地：{published:?}");
    let frame = std::fs::read_to_string(&published[0]).unwrap();
    assert!(
        frame.starts_with("@@workbuddy-shift-hook-source"),
        "{frame}"
    );
    assert!(frame.contains(payload), "{frame}");
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn concurrent_script_runs_publish_every_event_intact() {
    let Some(interpreter) = interpreter() else {
        eprintln!("跳过：本机没有可用于执行的 sh / bash");
        return;
    };
    const WRITERS: usize = 16;
    let home = temp_home("hook-script-concurrent");
    let script = ensure(&home, "workbuddy", "domestic").unwrap();
    let blob = "x".repeat(8 * 1024);
    let mut inputs = Vec::new();
    for id in 0..WRITERS {
        let path = home.join(format!("in-{id}.json"));
        std::fs::write(
            &path,
            format!("{{\"session_id\":\"c-{id}\",\"blob\":\"{blob}\"}}"),
        )
        .unwrap();
        inputs.push(path);
    }
    // 16 个进程真正同时起跑：一事件一文件，物理上不可能交错。
    let children: Vec<_> = inputs
        .iter()
        .map(|path| {
            std::process::Command::new(interpreter)
                .arg(script.to_string_lossy().replace('\\', "/"))
                .stdin(std::process::Stdio::from(
                    std::fs::File::open(path).unwrap(),
                ))
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap()
        })
        .collect();
    for mut child in children {
        assert!(child.wait().unwrap().success());
    }
    let mut reader = crate::HookSpool::new(&home);
    let mut ids = Vec::new();
    reader
        .drain(|value| {
            assert_eq!(value["blob"].as_str().map(str::len), Some(blob.len()));
            ids.push(
                value["session_id"]
                    .as_str()
                    .and_then(|id| id.trim_start_matches("c-").parse::<usize>().ok())
                    .unwrap_or(usize::MAX),
            );
        })
        .unwrap();
    ids.sort_unstable();
    assert_eq!(
        ids,
        (0..WRITERS).collect::<Vec<usize>>(),
        "并发写入不得丢事件"
    );
    for path in inputs {
        std::fs::remove_file(path).unwrap();
    }
    std::fs::remove_dir_all(&home).unwrap();
}

//! QA 第 2 轮独立验证（fresh eyes）：安装 / 迁移 / 卸载 / 纯函数不变量。
//!
//! 与 `integrations/tests.rs`（工程师用例）分开存放，断言独立构造：
//! 1. 注册命令是 `bash '<脚本>'`、timeout 一律 3 秒、exe 不再出现；
//! 2. 三种遗留形态（v0.4.2 的 exe 命令 ± `--edition`、旧 `.agent-studio`）迁移后
//!    只剩 1 条脚本命令，且重复安装幂等；
//! 3. 卸载：三形态清空 + 脚本删除 + opt-out 落盘 + 重启不重装；
//! 4. 红线：积分签到的 `bash '…/hook.sh'` 与 `cmd /c "…hook.cmd"`、第三方 hook 原样保留。

use super::*;
use companion_core::adapters::WORKBUDDY_HOOK_EVENTS;

/// 只有本工具会在这个专属目录里写脚本（与 `SCRIPT_DIR` 一致）。
const SCRIPT_DIR: &str = ".workbuddy-shift-companion/hooks";

fn seeded_home(tag: &str) -> PathBuf {
    let home = std::env::temp_dir().join(format!("qa-r2-{tag}-{}", uuid::Uuid::new_v4()));
    for dir in [".workbuddy", ".workbuddy-ai", ".codebuddy", ".codex"] {
        std::fs::create_dir_all(home.join(dir)).unwrap();
    }
    std::fs::write(home.join(".workbuddy/settings.json"), "{}").unwrap();
    std::fs::write(home.join(".workbuddy-ai/settings.json"), "{}").unwrap();
    home
}

/// 配置里所有出现过的 hook 命令（含分组内嵌条目）。
fn all_commands(doc: &Value) -> Vec<String> {
    doc["hooks"]
        .as_object()
        .into_iter()
        .flat_map(|map| map.values())
        .flat_map(|groups| groups.as_array().into_iter().flatten())
        .flat_map(|group| {
            group["hooks"]
                .as_array()
                .into_iter()
                .flatten()
                .chain(std::iter::once(group))
        })
        .filter_map(|hook| hook["command"].as_str().map(str::to_string))
        .collect()
}

fn is_ours(command: &str) -> bool {
    command.contains("companion-runtime")
        || command.contains("agent-studio-runtime")
        || command.contains(SCRIPT_DIR)
}

/// v0.4.2 注册进用户机器的 exe 命令（用户快照里就是这 10 条）。
fn v042_command(home: &Path, source: &str, edition: Option<&str>) -> String {
    hook_command(
        &binary(&Collector::new(home.to_path_buf()).unwrap()),
        home,
        source,
        edition,
    )
}

#[test]
fn qa_registered_command_is_a_script_with_a_three_second_timeout() {
    let home = seeded_home("timeout");
    let mut c = Collector::new(home.clone()).unwrap();
    set(&mut c, &json!({"source":"workbuddy","action":"install"})).unwrap();
    for (file, edition) in [
        (".workbuddy/settings.json", "domestic"),
        (".workbuddy-ai/settings.json", "international"),
    ] {
        let doc = read(&home.join(file)).unwrap();
        assert!(
            !doc.to_string().contains("companion-runtime"),
            "{file} 不得再注册 exe：{doc}"
        );
        for event in WORKBUDDY_HOOK_EVENTS {
            let installed: Vec<&Value> = doc["hooks"][event]
                .as_array()
                .unwrap_or_else(|| panic!("{file} 缺少事件 {event}"))
                .iter()
                .flat_map(|group| group["hooks"].as_array().into_iter().flatten())
                .filter(|hook| {
                    hook["command"]
                        .as_str()
                        .is_some_and(|cmd| cmd.contains(SCRIPT_DIR))
                })
                .collect();
            assert_eq!(installed.len(), 1, "{file}/{event} 必须恰好一条脚本命令");
            assert_eq!(
                installed[0]["timeout"], 3,
                "{file}/{event} 的 timeout 必须是 3 秒（1 秒是 issue #20 的回归）"
            );
            let command = installed[0]["command"].as_str().unwrap();
            assert!(
                command.starts_with("bash '") || command.starts_with("sh '"),
                "注册命令必须是 <解释器> '<脚本>'：{command}"
            );
            assert!(
                command.contains(&format!("hook-workbuddy-{edition}.sh")),
                "{file} 的应用版本写错了脚本：{command}"
            );
            if event == "SessionEnd" {
                assert!(installed[0]["async"].is_null(), "SessionEnd 不得异步");
            } else {
                assert_eq!(installed[0]["async"], true, "{event} 必须异步");
            }
        }
    }
    // 两个版本各一份脚本，正文里的来源字面量不同（不能串写）。
    let domestic =
        std::fs::read_to_string(hook_script::script_path(&home, "workbuddy", "domestic")).unwrap();
    let international = std::fs::read_to_string(hook_script::script_path(
        &home,
        "workbuddy",
        "international",
    ))
    .unwrap();
    assert!(domestic.contains("'workbuddy' 'domestic'"));
    assert!(international.contains("'workbuddy' 'international'"));
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn qa_install_migrates_three_legacy_forms_and_keeps_red_lines() {
    let home = seeded_home("migrate");
    let file = home.join(".workbuddy/settings.json");
    let rate_limit_bash = "bash '/c/Users/example/.workbuddy-shift/hook.sh'".to_string();
    let rate_limit_cmd = "cmd /c \"C:\\Users\\example\\.workbuddy-shift\\hook.cmd\"".to_string();
    let third_party = "node '/c/Users/example/plugins/tracker/index.js'".to_string();
    let legacy_studio = format!(
        "'{}' hook --home '{}' --source workbuddy",
        home.join(".agent-studio/bin/agent-studio-runtime-v1")
            .display(),
        home.display()
    );
    let mut c = Collector::new(home.clone()).unwrap();
    atomic_json(
        &file,
        &json!({
            "custom": true,
            "hooks": {
                "Stop": [{
                    "matcher": "",
                    "metadata": "keep",
                    "hooks": [
                        {"command": v042_command(&home, "workbuddy", Some("domestic"))},
                        {"command": v042_command(&home, "workbuddy", Some("international"))},
                        {"command": v042_command(&home, "workbuddy", None)},
                        {"command": legacy_studio},
                        {"command": third_party},
                        {"command": rate_limit_bash},
                        {"command": rate_limit_cmd}
                    ]
                }]
            }
        }),
    )
    .unwrap();
    set(&mut c, &json!({"source":"workbuddy","action":"install"})).unwrap();
    // 幂等：再装一次不得追加第二条。
    set(&mut c, &json!({"source":"workbuddy","action":"install"})).unwrap();

    let doc = read(&file).unwrap();
    // Stop 事件原本有 4 条遗留形态（±--edition 的 exe 命令、无 edition、旧品牌），
    // 迁移后必须恰好剩 1 条脚本命令；其余事件同理。
    for event in WORKBUDDY_HOOK_EVENTS {
        let ours: Vec<String> = doc["hooks"][event]
            .as_array()
            .unwrap_or_else(|| panic!("缺少事件 {event}"))
            .iter()
            .flat_map(|group| group["hooks"].as_array().into_iter().flatten())
            .filter_map(|hook| hook["command"].as_str())
            .filter(|cmd| is_ours(cmd))
            .map(str::to_string)
            .collect();
        assert_eq!(ours.len(), 1, "{event} 迁移后只能剩 1 条，实际：{ours:#?}");
        assert!(
            ours[0].contains("hook-workbuddy-domestic.sh"),
            "{event}: {}",
            ours[0]
        );
    }
    let all = all_commands(&doc);
    for keep in [&third_party, &rate_limit_bash, &rate_limit_cmd] {
        assert!(all.contains(keep), "红线命令被误删：{keep}\n全部：{all:#?}");
    }
    assert_eq!(doc["hooks"]["Stop"][0]["metadata"], "keep");
    assert_eq!(doc["custom"], true);
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn qa_uninstall_clears_three_forms_deletes_scripts_and_blocks_reinstall() {
    let home = seeded_home("uninstall");
    let file = home.join(".workbuddy/settings.json");
    let mut c = Collector::new(home.clone()).unwrap();
    set(&mut c, &json!({"source":"workbuddy","action":"install"})).unwrap();
    // 装完之后又冒出一条 v0.4.2 残留 + 一条积分签到 hook（红线）。
    let mut doc = read(&file).unwrap();
    doc["hooks"]["Stop"].as_array_mut().unwrap().push(json!({
        "matcher":"",
        "hooks":[
            {"command": v042_command(&home, "workbuddy", Some("domestic"))},
            {"command": "bash '/c/Users/example/.workbuddy-shift/hook.sh'"}
        ]
    }));
    atomic_json(&file, &doc).unwrap();
    assert!(hook_script::script_path(&home, "workbuddy", "domestic").is_file());

    let mut c = Collector::new(home.clone()).unwrap();
    uninstall_all_hooks(&mut c).unwrap();

    let after = read(&file).unwrap();
    let all = all_commands(&after);
    assert!(
        !after.to_string().contains("companion-runtime"),
        "v0.4.2 残留未清：{after}"
    );
    assert!(
        !all.iter().any(|cmd| is_ours(cmd)),
        "脚本命令未清：{all:#?}"
    );
    assert!(
        all.contains(&"bash '/c/Users/example/.workbuddy-shift/hook.sh'".to_string()),
        "积分签到 hook.sh 必须保留：{all:#?}"
    );
    for edition in ["domestic", "international"] {
        assert!(
            !hook_script::script_path(&home, "workbuddy", edition).exists(),
            "{edition} 脚本必须删除"
        );
    }
    assert!(
        !hook_script::scripts_dir(&home).exists(),
        "空脚本目录必须删除"
    );
    let opt_out: Value = serde_json::from_slice(
        &std::fs::read(home.join(".workbuddy-shift-companion/integrations.json")).unwrap(),
    )
    .unwrap();
    for source in ["codex", "workbuddy", "codebuddy-ide"] {
        assert_eq!(opt_out[source], false, "{source} 必须 opt-out");
    }
    // 重启：新 Collector + install_hooks（enabled 默认 true）不得重装。
    let c2 = Collector::new(home.clone()).unwrap();
    assert_eq!(c2.settings["sources"]["workbuddy"]["enabled"], true);
    install_hooks(&c2).unwrap();
    assert!(
        !hook_script::script_path(&home, "workbuddy", "domestic").exists(),
        "opt-out 后重启不得重装脚本"
    );
    assert_eq!(get(&c2)["sources"][1]["automatic"], false);
    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn qa_codex_still_registers_the_executable_with_three_seconds() {
    // codex 的结束事件需要同步确认持久化，必须留在 exe 路径上（不能被脚本化误伤）。
    let home = seeded_home("codex");
    let mut c = Collector::new(home.clone()).unwrap();
    set(&mut c, &json!({"source":"codex","action":"install"})).unwrap();
    let doc = read(&home.join(".codex/hooks.json")).unwrap();
    let commands = all_commands(&doc);
    assert!(
        commands.iter().all(|cmd| cmd.contains("companion-runtime")),
        "codex 仍必须走 exe：{commands:#?}"
    );
    assert!(
        commands.iter().all(|cmd| !cmd.contains(SCRIPT_DIR)),
        "codex 不得被脚本化：{commands:#?}"
    );
    for event in events("codex") {
        for group in doc["hooks"][event].as_array().unwrap() {
            for hook in group["hooks"].as_array().unwrap() {
                assert_eq!(hook["timeout"], 3, "{event} 的 timeout 必须是 3");
            }
        }
    }
    std::fs::remove_dir_all(&home).unwrap();
}

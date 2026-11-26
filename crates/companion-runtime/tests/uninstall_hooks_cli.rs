//! `companion-runtime uninstall-hooks` 的端到端回归：CLI 必须在没有 daemon 的情况下
//! 独立摘除注册项、写 opt-out，并且绝不碰积分签到的 hook.sh（issue #20）。

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

fn temp_home() -> PathBuf {
    let home =
        std::env::temp_dir().join(format!("companion-uninstall-cli-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(home.join(".workbuddy")).unwrap();
    std::fs::create_dir_all(home.join(".codex")).unwrap();
    home
}

fn hook_binary(home: &Path) -> PathBuf {
    home.join(if cfg!(windows) {
        ".workbuddy-shift-companion/bin/companion-runtime-v1.exe"
    } else {
        ".workbuddy-shift-companion/bin/companion-runtime-v1"
    })
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn uninstall_hooks_cli_clears_registration_and_blocks_reinstall() {
    let home = temp_home();
    let settings = home.join(".workbuddy/settings.json");
    let hook_sh = "bash '/c/Users/example/.workbuddy-shift/hook.sh'";
    let ours = format!(
        "'{}' hook --home '{}' --source workbuddy --edition domestic",
        hook_binary(&home).display(),
        home.display()
    );
    std::fs::write(
        &settings,
        json!({
            "custom": true,
            "hooks": {
                "Stop": [{
                    "matcher": "",
                    "metadata": "keep",
                    "hooks": [
                        {"type":"command","command":ours},
                        {"type":"command","command":hook_sh}
                    ]
                }]
            }
        })
        .to_string(),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_companion-runtime"))
        .args(["uninstall-hooks", "--home"])
        .arg(&home)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "CLI 失败：{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("已移除"), "stdout = {stdout}");

    let after = std::fs::read_to_string(&settings).unwrap();
    assert!(!after.contains("companion-runtime"), "{after}");
    assert!(after.contains("hook.sh"), "积分签到 hook 必须保留：{after}");
    let doc = read_json(&settings);
    assert_eq!(doc["custom"], true);
    assert_eq!(doc["hooks"]["Stop"][0]["metadata"], "keep");
    assert_eq!(doc["hooks"]["Stop"][0]["hooks"][0]["command"], hook_sh);

    let integrations = read_json(&home.join(".workbuddy-shift-companion/integrations.json"));
    for source in ["codex", "workbuddy", "codebuddy-ide"] {
        assert_eq!(integrations[source], false, "{source} 必须 opt-out");
    }

    // hook 自身必须即发即走：只输出 `{}` 并落盘，不得等待 IPC 或重写注册项。
    let second = Command::new(env!("CARGO_BIN_EXE_companion-runtime"))
        .args(["hook", "--home"])
        .arg(&home)
        .args(["--source", "workbuddy"])
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(second.status.success(), "hook 不应拦截宿主");
    assert_eq!(String::from_utf8_lossy(&second.stdout).trim(), "{}");
    assert!(!std::fs::read_to_string(&settings)
        .unwrap()
        .contains("companion-runtime"));

    std::fs::remove_dir_all(&home).unwrap();
}

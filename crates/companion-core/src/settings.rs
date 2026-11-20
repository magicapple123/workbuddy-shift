use serde_json::{json, Value};
use std::path::{Path, PathBuf};
pub const SOURCES: [&str; 5] = ["codex", "workbuddy", "codebuddy-ide", "codeg", "dsh"];
pub fn defaults() -> Value {
    let mut sources: serde_json::Map<String, Value> = SOURCES
        .iter()
        .map(|s| (s.to_string(), json!({"enabled":true,"path":""})))
        .collect();
    // WorkBuddy 沙箱审批不发送 hook；默认经由运行日志观察补充「待确认」信号。
    if let Some(workbuddy) = sources.get_mut("workbuddy") {
        workbuddy["logWatch"] = json!(true);
    }
    json!({"version":1,"sources":sources,"monitor":{"avatarStyle":"animal","railVisibleCount":8,"autoDiscover":true,"retentionHours":0.5,"assignment":"auto","seats":["auto","auto","auto","auto","auto","auto","auto","auto"]},"scene":{"light":"day","weather":"clear","lightning":true,"door":false,"ceiling":false,"playing":true,"speed":1,"maxFps":60,"renderResolution":"native","showPerformance":false,"reducedMotion":false,"defaultView":"program"},"notifications":{"desktop":false,"wait":true,"error":true,"done":true,"sound":false},"general":{"mode":"live","rememberView":true},"schedule":{"enabled":false,"start":"09:00","end":"18:00","deferBusy":true}})
}
pub fn validate(v: &Value) -> Result<Value, String> {
    let mut d = defaults();
    if v["version"] != 1 {
        return Err("配置版本无效".into());
    }
    for id in SOURCES {
        let p = v["sources"][id]["path"].as_str().ok_or("Agent 路径无效")?;
        if !v["sources"][id]["enabled"].is_boolean()
            || p.len() > 2048
            || p.contains('\0')
            || (!p.trim().is_empty()
                && !p.trim().starts_with("~/")
                && !Path::new(p.trim()).is_absolute())
        {
            return Err("Agent 配置无效".into());
        }
        let mut entry = json!({"enabled":v["sources"][id]["enabled"],"path":p.trim()});
        if id == "workbuddy" {
            let log_watch = &v["sources"][id]["logWatch"];
            if !log_watch.is_null() && !log_watch.is_boolean() {
                return Err("Agent 配置无效".into());
            }
            entry["logWatch"] = json!(log_watch.as_bool().unwrap_or(true));
        }
        d["sources"][id] = entry;
    }
    for group in ["monitor", "scene", "notifications", "general", "schedule"] {
        for (key, default) in defaults()[group].as_object().unwrap() {
            if group == "scene"
                && [
                    "maxFps",
                    "renderResolution",
                    "showPerformance",
                    "weather",
                    "lightning",
                ]
                .contains(&key.as_str())
                && v[group].get(key).is_none()
            {
                continue;
            }
            if group == "monitor"
                && ["railVisibleCount", "avatarStyle"].contains(&key.as_str())
                && v[group].get(key).is_none()
            {
                continue;
            }
            let value = &v[group][key];
            if value.is_null() || (default.is_boolean() && !value.is_boolean()) {
                return Err("配置分组或开关无效".into());
            }
            d[group][key] = value.clone();
        }
    }
    for (g, k, choices) in [
        ("monitor", "avatarStyle", json!(["animal", "bot"])),
        (
            "monitor",
            "railVisibleCount",
            json!([3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]),
        ),
        ("monitor", "retentionHours", json!([0, 0.5, 24, 168])),
        ("monitor", "assignment", json!(["auto", "fixed"])),
        ("scene", "light", json!(["day", "night", "auto"])),
        (
            "scene",
            "weather",
            json!([
                "clear",
                "overcast",
                "rain",
                "downpour",
                "thunderstorm",
                "wind",
                "auto"
            ]),
        ),
        ("scene", "speed", json!([1, 2, 4])),
        ("scene", "maxFps", json!([30, 60])),
        (
            "scene",
            "renderResolution",
            json!(["native", "balanced", "low"]),
        ),
        ("scene", "defaultView", json!(["all", "program", "device"])),
        ("general", "mode", json!(["live", "demo"])),
    ] {
        if !choices.as_array().unwrap().contains(&d[g][k]) {
            return Err("配置选项无效".into());
        }
    }
    let seats = d["monitor"]["seats"].as_array().ok_or("工位配置无效")?;
    if seats.len() != 8
        || seats
            .iter()
            .any(|s| s != "auto" && !SOURCES.contains(&s.as_str().unwrap_or("")))
    {
        return Err("工位配置无效".into());
    }
    let time = |v: &Value| -> Option<u16> {
        let s = v.as_str()?;
        let (h, m) = s.split_once(':')?;
        if h.len() != 2 || m.len() != 2 {
            return None;
        }
        let h = h.parse::<u16>().ok()?;
        let m = m.parse::<u16>().ok()?;
        if h > 23 || m > 59 {
            None
        } else {
            Some(h * 60 + m)
        }
    };
    if !matches!((time(&d["schedule"]["start"]),time(&d["schedule"]["end"])),(Some(a),Some(b)) if a<b)
    {
        return Err("上班时间必须早于下班时间".into());
    }
    Ok(d)
}
/// `dsh_home` 的纯函数版本：不读环境变量，便于单测。
///
/// `~/.dsh` 只是缺省值；DSH 自己用 `$DSH_HOME` 定位数据目录，所以监听侧必须
/// 支持同一个变量，否则用户改了 DSH_HOME 后我们永远读到空目录。
pub fn resolve_dsh_home(home: &Path, dsh_home: Option<&str>) -> PathBuf {
    match dsh_home.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => match p.strip_prefix("~/") {
            Some(rest) => home.join(rest),
            None => PathBuf::from(p),
        },
        None => home.join(".dsh"),
    }
}

/// DSH（DeepSeek Harness）的数据根目录：`$DSH_HOME` 优先，缺省 `~/.dsh`。
pub fn dsh_home(home: &Path) -> PathBuf {
    resolve_dsh_home(home, std::env::var("DSH_HOME").ok().as_deref())
}

pub fn paths(home: &Path, v: &Value, source: &str) -> Vec<PathBuf> {
    if let Some(p) = v["sources"][source]["path"]
        .as_str()
        .filter(|p| !p.is_empty())
    {
        if source == "codebuddy-ide" && p.ends_with(".vscdb") {
            return vec![home.join(".codebuddy"), home.join(".codebuddycn")];
        }
        return vec![if let Some(s) = p.strip_prefix("~/") {
            home.join(s)
        } else {
            p.into()
        }];
    }
    match source {
        "codex" => vec![home.join(".codex")],
        "workbuddy" => vec![home.join(".workbuddy-ai"), home.join(".workbuddy")],
        "codebuddy-ide" => vec![home.join(".codebuddy"), home.join(".codebuddycn")],
        "codeg" => [
            "Library/Application Support/app.codeg/codeg.db",
            "Library/Application Support/codeg/codeg.db",
            ".local/share/codeg/codeg.db",
        ]
        .map(|p| home.join(p))
        .to_vec(),
        // DSH 只读会话日志；根目录下由适配器再找 `sessions/`。
        "dsh" => vec![dsh_home(home)],
        _ => vec![home.join(".codebuddy")],
    }
}

#[cfg(test)]
mod frame_rate_tests {
    use super::*;
    #[test]
    fn avatar_style_migrates_and_validates() {
        let mut value = defaults();
        value["monitor"]
            .as_object_mut()
            .unwrap()
            .remove("avatarStyle");
        assert_eq!(
            validate(&value).unwrap()["monitor"]["avatarStyle"],
            "animal"
        );
        for mode in ["animal", "bot"] {
            value["monitor"]["avatarStyle"] = json!(mode);
            assert_eq!(validate(&value).unwrap()["monitor"]["avatarStyle"], mode);
        }
        value["monitor"]["avatarStyle"] = json!("invalid");
        assert!(validate(&value).is_err());
    }
    #[test]
    fn render_resolution_migrates_and_validates() {
        let mut value = defaults();
        value["scene"]
            .as_object_mut()
            .unwrap()
            .remove("renderResolution");
        assert_eq!(
            validate(&value).unwrap()["scene"]["renderResolution"],
            "native"
        );
        for mode in ["native", "balanced", "low"] {
            value["scene"]["renderResolution"] = json!(mode);
            assert_eq!(validate(&value).unwrap()["scene"]["renderResolution"], mode);
        }
        value["scene"]["renderResolution"] = json!("invalid");
        assert!(validate(&value).is_err());
    }
    #[test]
    fn old_settings_default_to_60_and_only_supported_rates_are_saved() {
        let mut value = defaults();
        value["scene"].as_object_mut().unwrap().remove("maxFps");
        assert_eq!(validate(&value).unwrap()["scene"]["maxFps"], 60);
        value["scene"]["maxFps"] = json!(60);
        assert_eq!(validate(&value).unwrap()["scene"]["maxFps"], 60);
        value["scene"]["maxFps"] = json!(120);
        assert!(validate(&value).is_err());
    }
    #[test]
    fn workbuddy_default_paths_include_current_and_legacy() {
        let home = Path::new("/tmp/studio-home");
        assert_eq!(
            paths(home, &defaults(), "workbuddy"),
            vec![home.join(".workbuddy-ai"), home.join(".workbuddy")]
        );
    }
    #[test]
    fn workbuddy_log_watch_defaults_on_and_validates_boolean() {
        let mut value = defaults();
        assert_eq!(value["sources"]["workbuddy"]["logWatch"], true);
        value["sources"]["workbuddy"]
            .as_object_mut()
            .unwrap()
            .remove("logWatch");
        assert_eq!(
            validate(&value).unwrap()["sources"]["workbuddy"]["logWatch"],
            true
        );
        value["sources"]["workbuddy"]["logWatch"] = json!(false);
        assert_eq!(
            validate(&value).unwrap()["sources"]["workbuddy"]["logWatch"],
            false
        );
        value["sources"]["workbuddy"]["logWatch"] = json!("yes");
        assert!(validate(&value).is_err());
    }
    #[test]
    fn dsh_paths_default_to_dot_dsh_and_honour_the_override() {
        let home = Path::new("/tmp/studio-home");
        // 缺省与空白 DSH_HOME 都回退 ~/.dsh。
        assert_eq!(resolve_dsh_home(home, None), home.join(".dsh"));
        assert_eq!(resolve_dsh_home(home, Some("   ")), home.join(".dsh"));
        // 与其它来源一致：`~/` 相对用户主目录展开。
        assert_eq!(
            resolve_dsh_home(home, Some("~/dsh-data")),
            home.join("dsh-data")
        );
        assert_eq!(
            resolve_dsh_home(home, Some("/opt/dsh-home")),
            PathBuf::from("/opt/dsh-home")
        );
        // 设置里手填的路径优先（与 codex/workbuddy 同一套规则）。
        let mut value = defaults();
        assert_eq!(value["sources"]["dsh"]["enabled"], true);
        value["sources"]["dsh"]["path"] = json!("~/custom-dsh");
        assert_eq!(paths(home, &value, "dsh"), vec![home.join("custom-dsh")]);
        // 工位可以钉在 DSH 上（校验用 SOURCES，定长数组改错会在这里挂）。
        value = defaults();
        value["monitor"]["seats"][0] = json!("dsh");
        assert_eq!(validate(&value).unwrap()["monitor"]["seats"][0], "dsh");
        assert_eq!(SOURCES.len(), 5);
        assert!(SOURCES.contains(&"dsh"));
    }
}

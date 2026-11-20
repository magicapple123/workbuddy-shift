//! 跨账号会话的**专家绑定**只读判定（R5-13）。
//!
//! 为什么需要：会话行里的 `expert_id` / `expert_runtime_identity` / `expert_marketplace` 会随
//! 整行复制搬到目标账号。内置与市场专家通常没问题（插件包全局安装、`marketExpertId` 与账号
//! 无关），**自建专家**在目标账号会悬空——而复制前唯一的拦截 `ensure_row_copyable` 只看
//! Claw 工作区，不看专家，于是用户拿到一条「打不开专家身份」的会话却没有任何提示。
//!
//! 本模块**只做判定**，不改写任何列、不复制专家包、不参与「跳过 / 拒绝」决策。三件事被明确
//! 禁止（写在这里防止后续「顺手优化」）：
//!
//! 1. 禁止清空或改写 `expert_id` / `expert_runtime_identity` / `expert_marketplace` /
//!    `last_user_prompt_expert_selection`；
//! 2. 禁止复制 `plugins/cache/experts/*` 或修改 `installed_plugins.json`；
//! 3. 禁止因判定为 `missing` 而跳过整条会话（默认仍勾选、仍复制）。
//!
//! 判定按 **pluginName** 而不是 `expert_id`：同一专家在会话里存在「市场实例 id」
//! （`ex_xxx`）与「规范 id」（`SoftwareCompany`）两种写法，按 id 比会大面积误判。

use std::path::Path;

use serde_json::Value;

/// 判定结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::modules::session) struct ExpertBinding {
    /// `available` / `missing` / `unknown`。
    pub(in crate::modules::session) state: &'static str,
    /// 插件注册名（如 `software-company`）。
    pub(in crate::modules::session) plugin: Option<String>,
    /// 展示名（identity 里的 `agentName`，缺失时用 `expertId`）。
    pub(in crate::modules::session) name: Option<String>,
}

impl ExpertBinding {
    fn unknown() -> Self {
        Self {
            state: "unknown",
            plugin: None,
            name: None,
        }
    }
}

/// 从 `expert-v2:{json}` 里取插件名与展示名；任何异常都返回 `None`（→ `unknown`）。
///
/// 宁可漏报也不误报：解析不出来就不提示，避免把正常会话标黄。
fn parse_identity(identity: &str) -> Option<(String, Option<String>)> {
    let json = identity.strip_prefix("expert-v2:").unwrap_or(identity);
    let value: Value = serde_json::from_str(json).ok()?;
    let plugin = value
        .get("pluginRegisteredName")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())?
        .to_string();
    let name = value
        .get("agentName")
        .and_then(Value::as_str)
        .or_else(|| value.get("expertId").and_then(Value::as_str))
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string);
    Some((plugin, name))
}

/// 目标账号是否见过这个插件（`storage/user-<uid>-personal/global/expert.json` 的 recents）。
fn in_account_recents(data_root: &Path, uid: &str, plugin: &str) -> bool {
    let path = data_root
        .join("storage")
        .join(format!("user-{uid}-personal"))
        .join("global")
        .join("expert.json");
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return false;
    };
    value
        .get("recents")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items.iter().any(|item| {
                item.get("pluginName")
                    .and_then(Value::as_str)
                    .is_some_and(|name| name == plugin)
            })
        })
}

/// 插件是否已安装（`plugins/installed_plugins.json` 里存在 `<plugin>@<marketplace>`）。
fn in_installed_plugins(data_root: &Path, plugin: &str) -> bool {
    let path = data_root.join("plugins").join("installed_plugins.json");
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return false;
    };
    let Some(map) = value
        .get("plugins")
        .and_then(Value::as_object)
        .or_else(|| value.as_object())
    else {
        return false;
    };
    let prefix = format!("{plugin}@");
    map.keys().any(|key| key.starts_with(&prefix))
}

/// 插件包是否在本地缓存里（`plugins/cache/experts/<plugin>/`，不校验版本）。
fn in_plugin_cache(data_root: &Path, plugin: &str) -> bool {
    data_root
        .join("plugins")
        .join("cache")
        .join("experts")
        .join(plugin)
        .is_dir()
}

/// 判定一条会话的专家在目标账号是否可用。
///
/// `data_root` 是**目标账号所属档位**的数据根（`WbVariant::data_root()`，如 `~/.workbuddy`）。
/// 目标账号从未用过专家（`expert.json` 不存在）时只按「已安装 / 已缓存」判断——插件装了就算
/// 可用，避免新账号大面积误报。
pub(in crate::modules::session) fn detect(
    data_root: &Path,
    uid: &str,
    expert_id: Option<&str>,
    identity: Option<&str>,
) -> ExpertBinding {
    let expert_id = expert_id.map(str::trim).filter(|text| !text.is_empty());
    let identity = identity.map(str::trim).filter(|text| !text.is_empty());
    if expert_id.is_none() || identity.is_none() {
        return ExpertBinding::unknown();
    }
    let Some((plugin, name)) = parse_identity(identity.unwrap()) else {
        return ExpertBinding::unknown();
    };
    let available = in_account_recents(data_root, uid, &plugin)
        || in_installed_plugins(data_root, &plugin)
        || in_plugin_cache(data_root, &plugin);
    ExpertBinding {
        state: if available { "available" } else { "missing" },
        plugin: Some(plugin),
        name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Env {
        root: PathBuf,
    }

    impl Env {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "wb_shift_expert_{}_{name}",
                uuid::Uuid::new_v4().simple()
            ));
            std::fs::create_dir_all(&root).unwrap();
            Env { root }
        }

        fn write(&self, rel: &str, text: &str) {
            let path = self.root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }

        fn mkdir(&self, rel: &str) {
            std::fs::create_dir_all(self.root.join(rel)).unwrap();
        }
    }

    impl Drop for Env {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    const IDENT: &str = r#"expert-v2:{"v":2,"kind":"expert","expertId":"SoftwareCompany","marketExpertId":"ex_jPdPfIC04OKD","pluginRegisteredName":"software-company","agentName":"SoftwareCompany","source":"builtin"}"#;

    #[test]
    fn empty_expert_fields_are_unknown() {
        let env = Env::new("empty");
        assert_eq!(
            detect(&env.root, "uid-b", None, Some(IDENT)).state,
            "unknown"
        );
        assert_eq!(detect(&env.root, "uid-b", Some("x"), None).state, "unknown");
        assert_eq!(
            detect(&env.root, "uid-b", Some(""), Some(" ")).state,
            "unknown"
        );
    }

    #[test]
    fn broken_identity_is_unknown_not_missing() {
        let env = Env::new("broken");
        let broken = "expert-v2:{not json";
        let binding = detect(&env.root, "uid-b", Some("ex_1"), Some(broken));
        assert_eq!(binding.state, "unknown", "解析失败必须不提示");
    }

    #[test]
    fn plugin_cache_means_available() {
        let env = Env::new("cache");
        env.mkdir("plugins/cache/experts/software-company/1.1.0");
        let binding = detect(&env.root, "uid-b", Some("ex_jPdPfIC04OKD"), Some(IDENT));
        assert_eq!(binding.state, "available");
        assert_eq!(binding.plugin.as_deref(), Some("software-company"));
        assert_eq!(binding.name.as_deref(), Some("SoftwareCompany"));
    }

    #[test]
    fn installed_plugins_means_available() {
        let env = Env::new("installed");
        env.write(
            "plugins/installed_plugins.json",
            r#"{"plugins":{"software-company@experts":{"enabled":true}}}"#,
        );
        assert_eq!(
            detect(&env.root, "uid-b", Some("SoftwareCompany"), Some(IDENT)).state,
            "available"
        );
    }

    #[test]
    fn account_recents_means_available_even_without_cache() {
        let env = Env::new("recents");
        env.write(
            "storage/user-uid-b-personal/global/expert.json",
            r#"{"recents":[{"id":"x","pluginName":"software-company"}]}"#,
        );
        assert_eq!(
            detect(&env.root, "uid-b", Some("SoftwareCompany"), Some(IDENT)).state,
            "available"
        );
    }

    #[test]
    fn nothing_installed_is_missing() {
        let env = Env::new("missing");
        let binding = detect(&env.root, "uid-b", Some("ex_1"), Some(IDENT));
        assert_eq!(binding.state, "missing");
        assert_eq!(binding.state, "missing");
        assert_eq!(binding.plugin.as_deref(), Some("software-company"));
    }

    /// 同一专家的两种 id 写法（市场实例 id / 规范 id）判定必须一致：按 pluginName 比。
    #[test]
    fn both_id_forms_resolve_the_same() {
        let env = Env::new("two-ids");
        env.mkdir("plugins/cache/experts/software-company");
        let by_market = detect(&env.root, "uid-b", Some("ex_jPdPfIC04OKD"), Some(IDENT));
        let by_canonical = detect(&env.root, "uid-b", Some("SoftwareCompany"), Some(IDENT));
        assert_eq!(by_market.state, "available");
        assert_eq!(by_market.state, by_canonical.state);
    }
}

//! 设置包导出 / 导入（四份自动化配置）。
//!
//! 为什么只有这四份：`checkin` / `travel` / `rotate` / `rateLimit` 是纯偏好，换机、重装、
//! 多台机器之间搬运都安全。**刻意不含**更新（GitHub）配置：那份带 `proxy` 等字段，明文导出
//! 等于把用户的代理链路（可能含账号口令）写进一个会被随手分享的文件；也不含账号库
//! （带 token）。导出文件里因此不存在任何 token / 密钥。
//!
//! 契约（见 `AGENTS.md` 第 3 节）：
//!
//! - 导出形状固定 `{ version, exportedAt, configs: { checkin, travel, rotate, rateLimit } }`；
//! - 导入按白名单校验：`configs` 里只认那四个键，每个键必须是对象且**字段齐全、类型正确**
//!   （半截对象、未知字段一律拒绝）；
//! - 任一键非法 → 整体拒绝、零写入（不半写）；
//! - 写入前把现有配置文件逐个复制到 `backups/settings-import-<毫秒>/`，写入失败时用备份回滚。

use super::*;
use crate::modules::locks;

/// 设置包版本：导入时严格比对，未来的形状变化靠提版本号 + 兼容分支处理。
pub const SETTINGS_BUNDLE_VERSION: u64 = 1;

/// 允许打包的四份配置。
///
/// 顺序是**导入报告**（`imported`）的顺序，也是导出时组装 `configs` 的顺序；
/// 但 JSON 对象本身不保证键序（`serde_json::Map` 在未开 `preserve_order` 时按字典序输出），
/// 因此导入侧一律按键取值，不依赖顺序。
pub const SETTINGS_BUNDLE_KEYS: [&str; 4] = ["checkin", "travel", "rotate", "rateLimit"];

/// 导入串行化守卫：两个导入同时写四份配置会互相覆盖备份与回滚记录。
static SETTINGS_BUNDLE_LOCK: Mutex<()> = Mutex::new(());

/// 四份配置与备份目录的实际路径。
///
/// 单测注入临时目录，生产走 `store_dir()` / `backup_dir()`；导出与导入共用同一份路径表，
/// 避免两边各写一套文件名。
pub(crate) struct BundlePaths {
    pub(crate) checkin_config: PathBuf,
    pub(crate) checkin_logs: PathBuf,
    pub(crate) travel_config: PathBuf,
    pub(crate) travel_cache: PathBuf,
    pub(crate) rotate_config: PathBuf,
    pub(crate) rate_limit_config: PathBuf,
    pub(crate) backup_root: PathBuf,
}

impl BundlePaths {
    /// 生产路径：全部落在工具数据目录下。
    pub(crate) fn from_store() -> Self {
        Self {
            checkin_config: checkin_config_file(),
            checkin_logs: checkin_logs_file(),
            travel_config: travel_config_file(),
            travel_cache: travel_cache_file(),
            rotate_config: auto_rotate_config_file(),
            rate_limit_config: rate_limit_config_file(),
            backup_root: backup_dir(),
        }
    }

    fn checkin_value(&self) -> Value {
        load_checkin_config_at(&self.checkin_config, &self.checkin_logs)
    }

    fn travel_value(&self) -> Value {
        load_travel_config_at(&self.travel_config, &self.travel_cache)
    }

    fn rotate_value(&self) -> Value {
        load_auto_rotate_config_at(&self.rotate_config)
    }

    fn rate_limit_value(&self) -> Value {
        load_rate_limit_config_at(&self.rate_limit_config)
    }
}

/// 单字段类型白名单：类型不符的导入在边界就被拒绝，不会被静默降级成默认值。
#[derive(Clone, Copy)]
enum FieldKind {
    Bool,
    Integer,
    Text,
    TextList,
}

impl FieldKind {
    fn matches(self, value: &Value) -> bool {
        match self {
            FieldKind::Bool => value.is_boolean(),
            FieldKind::Integer => value.as_i64().is_some(),
            FieldKind::Text => value.is_string(),
            FieldKind::TextList => value
                .as_array()
                .is_some_and(|items| items.iter().all(Value::is_string)),
        }
    }
}

/// 每份配置的字段白名单（字段名与各 `save_*` 认可的键逐字一致）。
fn field_spec(key: &str) -> &'static [(&'static str, FieldKind)] {
    const CHECKIN: &[(&str, FieldKind)] = &[
        ("enabled", FieldKind::Bool),
        ("excluded_account_ids", FieldKind::TextList),
        ("checkin_start", FieldKind::Text),
        ("checkin_end", FieldKind::Text),
        ("start_hour", FieldKind::Integer),
        ("end_hour", FieldKind::Integer),
        ("keepalive_days", FieldKind::Integer),
        ("lazy_refresh_hours", FieldKind::Integer),
    ];
    const TRAVEL: &[(&str, FieldKind)] = &[("enabled", FieldKind::Bool)];
    const ROTATE: &[(&str, FieldKind)] = &[
        ("enabled", FieldKind::Bool),
        ("check_interval_minutes", FieldKind::Integer),
        ("cooldown_minutes", FieldKind::Integer),
        ("min_gap_hours", FieldKind::Integer),
        ("min_urgency_hours", FieldKind::Integer),
        ("active_guard_minutes", FieldKind::Integer),
        ("min_remaining_credits", FieldKind::Integer),
    ];
    const RATE_LIMIT: &[(&str, FieldKind)] = &[
        ("enabled", FieldKind::Bool),
        ("hookOptOut", FieldKind::Bool),
        ("scanIdeLogs", FieldKind::Bool),
        ("crossClientInfer", FieldKind::Bool),
    ];
    match key {
        "checkin" => CHECKIN,
        "travel" => TRAVEL,
        "rotate" => ROTATE,
        _ => RATE_LIMIT,
    }
}

/// 该配置的出厂默认值（导出时补缺、导入前的归一化基线）。
fn default_for(key: &str) -> Value {
    match key {
        "checkin" => default_checkin_config(),
        "travel" => default_travel_config(),
        "rotate" => default_auto_rotate_config(),
        _ => default_rate_limit_config(),
    }
}

fn target_path<'a>(paths: &'a BundlePaths, key: &str) -> &'a Path {
    match key {
        "checkin" => &paths.checkin_config,
        "travel" => &paths.travel_config,
        "rotate" => &paths.rotate_config,
        _ => &paths.rate_limit_config,
    }
}

/// 备份目录里的文件名：与数据目录里的正式文件名一致，用户能直接认出来。
fn backup_name(key: &str) -> &'static str {
    match key {
        "checkin" => "auto_checkin_config.json",
        "travel" => "auto_travel_config.json",
        "rotate" => "auto_rotate_config.json",
        _ => "rate_limit_config.json",
    }
}

/// 按字段白名单从 `source` 取已知字段，缺失的用默认值补（导出用）。
///
/// 只取白名单里的键：即使磁盘上的配置被手工塞了额外字段，也不会被带进导出文件。
fn pick_config(key: &str, source: &Value) -> Value {
    let mut out = default_for(key);
    if let Some(map) = source.as_object() {
        for (name, _) in field_spec(key) {
            if let Some(value) = map.get(*name) {
                out[*name] = value.clone();
            }
        }
    }
    out
}

/// 落盘前的归一化：与各 `save_*` 的字段规则保持一致（时间段零填充、排除名单去重）。
fn normalized_for_write(key: &str, cfg: &Value) -> Value {
    if key == "checkin" {
        // 复用保存路径的归一化，避免「导出再导入」与「设置页保存」写出两种形状。
        return merge_checkin_config(cfg);
    }
    pick_config(key, cfg)
}

/// 校验单个配置对象：字段必须齐全且类型正确，未知字段一律拒绝。
fn validate_config(key: &str, value: &Value) -> Result<Value, String> {
    let Some(map) = value.as_object() else {
        return Err(format!("设置包里的 {key} 必须是 JSON 对象"));
    };
    let spec = field_spec(key);
    for (name, _) in spec {
        if !map.contains_key(*name) {
            return Err(format!(
                "设置包里的 {key} 缺少字段 {name}（不完整的配置），已拒绝导入"
            ));
        }
    }
    for (name, field) in map {
        match spec.iter().find(|(known, _)| *known == name.as_str()) {
            Some((_, kind)) => {
                if !kind.matches(field) {
                    return Err(format!("设置包里的 {key}.{name} 类型不正确，已拒绝导入"));
                }
            }
            None => {
                return Err(format!(
                    "设置包里的 {key}.{name} 不是可导入的字段，已拒绝导入"
                ));
            }
        }
    }
    Ok(Value::Object(map.clone()))
}

/// 解析并校验整份设置包；返回待写入的 `(键, 归一化后的配置)` 列表。
///
/// 只允许包含部分键（用户可能只想搬其中一两份），但**每个出现的键都必须完整合法**。
fn parse_bundle(bundle: &Value) -> Result<Vec<(&'static str, Value)>, String> {
    let Some(root) = bundle.as_object() else {
        return Err("设置包必须是 JSON 对象".to_string());
    };
    if let Some(version) = root.get("version") {
        match version.as_u64() {
            Some(v) if v == SETTINGS_BUNDLE_VERSION => {}
            _ => {
                return Err(format!(
                    "设置包版本不受支持（本版本只接受 version = {SETTINGS_BUNDLE_VERSION}）"
                ))
            }
        }
    }
    let Some(configs) = root.get("configs").and_then(Value::as_object) else {
        return Err("设置包缺少 configs 对象".to_string());
    };
    if configs.is_empty() {
        return Err("设置包的 configs 为空，没有可导入的配置".to_string());
    }
    for name in configs.keys() {
        if !SETTINGS_BUNDLE_KEYS.contains(&name.as_str()) {
            return Err(format!("设置包里出现未知配置项 {name}，已拒绝导入"));
        }
    }
    let mut entries = Vec::new();
    // 按 `SETTINGS_BUNDLE_KEYS` 的**声明顺序**遍历，逐个 `get`：JSON 对象本身不保证键序
    // （`serde_json::Map` 未开 `preserve_order` 时是 `BTreeMap`，键序恒为字节序），
    // 若改成 `for (key, value) in configs` 就会被字典序带走，导入报告的顺序随之变得不可断言。
    for key in SETTINGS_BUNDLE_KEYS {
        let Some(value) = configs.get(key) else {
            continue;
        };
        validate_config(key, value)?;
        entries.push((key, normalized_for_write(key, value)));
    }
    Ok(entries)
}

/// 导出设置包（生产路径）。
pub fn export_settings_bundle() -> Value {
    export_settings_bundle_at(&BundlePaths::from_store(), now_ms())
}

/// 导出设置包（可注入路径与时间，供单测使用）。
pub(crate) fn export_settings_bundle_at(paths: &BundlePaths, exported_at: i64) -> Value {
    json!({
        "version": SETTINGS_BUNDLE_VERSION,
        "exportedAt": exported_at,
        "configs": {
            "checkin": pick_config("checkin", &paths.checkin_value()),
            "travel": pick_config("travel", &paths.travel_value()),
            "rotate": pick_config("rotate", &paths.rotate_value()),
            "rateLimit": pick_config("rateLimit", &paths.rate_limit_value()),
        },
    })
}

/// 导入设置包（生产路径）。
pub fn import_settings_bundle(bundle: &Value) -> Result<Value, String> {
    import_settings_bundle_at(&BundlePaths::from_store(), bundle, now_ms())
}

/// 导入设置包（可注入路径与时间，供单测使用）。
pub(crate) fn import_settings_bundle_at(
    paths: &BundlePaths,
    bundle: &Value,
    at_ms: i64,
) -> Result<Value, String> {
    // 先全量校验：任何一项非法都在这里返回，配置文件一个字节都不会动。
    let entries = parse_bundle(bundle)?;
    // 纯串行化守卫（`Mutex<()>`）：中毒不携带数据，取回后继续——回滚记录在本次调用的栈上。
    let _guard = locks::recover(&SETTINGS_BUNDLE_LOCK);

    let backup_dir = paths.backup_root.join(format!("settings-import-{at_ms}"));
    std::fs::create_dir_all(&backup_dir).map_err(|e| format!("创建配置备份目录失败：{e}"))?;

    // 1) 逐份备份：不存在的文件记 None，回滚时按「原本没有」处理（删掉半写产物）。
    let mut snapshots: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
    for (key, _) in &entries {
        let key = *key;
        let path = target_path(paths, key).to_path_buf();
        if path.exists() {
            let copy = backup_dir.join(backup_name(key));
            std::fs::copy(&path, &copy).map_err(|e| format!("备份 {key} 配置失败：{e}"))?;
            snapshots.push((path, Some(copy)));
        } else {
            snapshots.push((path, None));
        }
    }

    // 2) 写入：任一失败就整体回滚，不留「一半新一半旧」的配置。
    let mut imported: Vec<&'static str> = Vec::new();
    for (key, cfg) in entries {
        let path = target_path(paths, key);
        let content = serde_json::to_string_pretty(&cfg)
            .map_err(|e| format!("序列化 {key} 配置失败：{e}"))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("创建数据目录失败：{e}"))?;
        }
        if let Err(e) = atomic_write(path, &content) {
            rollback(&snapshots);
            return Err(format!("写入 {key} 配置失败，已回滚全部改动：{e}"));
        }
        imported.push(key);
    }

    Ok(json!({
        "imported": imported,
        "backupDir": backup_dir.to_string_lossy(),
        "importedAt": at_ms,
    }))
}

/// 用写入前的快照还原：有备份的复制回去，原本不存在的删除。
fn rollback(snapshots: &[(PathBuf, Option<PathBuf>)]) {
    for (path, backup) in snapshots {
        match backup {
            Some(copy) => {
                let _ = std::fs::copy(copy, path);
            }
            None => {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 临时沙箱：四份配置 + 备份目录。每个用例一份，避免并行测试互相干扰。
    fn temp_paths(label: &str) -> BundlePaths {
        let dir = std::env::temp_dir().join(format!(
            "workbuddy-shift-settings-bundle-{label}-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).expect("创建临时目录");
        BundlePaths {
            checkin_config: dir.join("auto_checkin_config.json"),
            checkin_logs: dir.join("auto_checkin_logs.json"),
            travel_config: dir.join("auto_travel_config.json"),
            travel_cache: dir.join("travel_cache.json"),
            rotate_config: dir.join("auto_rotate_config.json"),
            rate_limit_config: dir.join("rate_limit_config.json"),
            backup_root: dir.join("backups"),
        }
    }

    fn read_bytes(path: &Path) -> Vec<u8> {
        std::fs::read(path).unwrap_or_default()
    }

    /// 写入一组「非默认」配置，确保往返测试不会因为默认值相同而假通过。
    fn seed(paths: &BundlePaths) {
        std::fs::write(
            &paths.checkin_config,
            serde_json::to_string_pretty(&json!({
                "enabled": true,
                "excluded_account_ids": ["acc-1", "acc-2"],
                "checkin_start": "08:30",
                "checkin_end": "10:15",
                "start_hour": 6,
                "end_hour": 12,
                "keepalive_days": 3,
                "lazy_refresh_hours": 12,
            }))
            .expect("序列化签到配置"),
        )
        .expect("写入签到配置");
        std::fs::write(&paths.travel_config, "{\"enabled\": true}").expect("写入旅行配置");
        std::fs::write(
            &paths.rotate_config,
            serde_json::to_string_pretty(&json!({
                "enabled": true,
                "check_interval_minutes": 7,
                "cooldown_minutes": 90,
                "min_gap_hours": 12,
                "min_urgency_hours": 48,
                "active_guard_minutes": 15,
                "min_remaining_credits": 120,
            }))
            .expect("序列化轮换配置"),
        )
        .expect("写入轮换配置");
        std::fs::write(
            &paths.rate_limit_config,
            serde_json::to_string_pretty(&json!({
                "enabled": false,
                "hookOptOut": true,
                "scanIdeLogs": false,
                "crossClientInfer": false,
            }))
            .expect("序列化限额配置"),
        )
        .expect("写入限额配置");
    }

    #[test]
    fn export_import_round_trip_keeps_four_configs() {
        let source = temp_paths("round-trip-source");
        seed(&source);
        let bundle = export_settings_bundle_at(&source, 1_700_000_000_000);

        assert_eq!(bundle["version"], json!(SETTINGS_BUNDLE_VERSION));
        assert_eq!(bundle["exportedAt"], json!(1_700_000_000_000_i64));
        let configs = bundle["configs"].as_object().expect("configs 必须是对象");
        // 只比集合、不比顺序：`serde_json::Map` 未开 preserve_order 时按键字典序输出，
        // 顺序不是契约的一部分（导入也都按键取值）。
        let mut exported_keys = configs.keys().cloned().collect::<Vec<_>>();
        exported_keys.sort();
        let mut expected_keys = SETTINGS_BUNDLE_KEYS
            .iter()
            .map(|key| key.to_string())
            .collect::<Vec<_>>();
        expected_keys.sort();
        assert_eq!(exported_keys, expected_keys);

        // 导入到另一份空沙箱：结果必须与导出的四份配置逐字一致。
        let target = temp_paths("round-trip-target");
        let report =
            import_settings_bundle_at(&target, &bundle, 1_700_000_100_000).expect("导入必须成功");
        // 报告顺序可以断言：`import_settings_bundle_at` 按 `SETTINGS_BUNDLE_KEYS` 的声明顺序
        // 逐个 `configs.get(key)` 推入，与 JSON 对象的键序无关。
        assert_eq!(
            report["imported"],
            json!(["checkin", "travel", "rotate", "rateLimit"])
        );

        let re_exported = export_settings_bundle_at(&target, 1_700_000_200_000);
        assert_eq!(re_exported["configs"], bundle["configs"]);
        // 备份目录落在备份根下，且包含四份原文件（目标侧原本不存在 → 备份为空目录）。
        assert!(target
            .backup_root
            .join("settings-import-1700000100000")
            .exists());
    }

    #[test]
    fn import_rejects_non_object_payload_without_touching_files() {
        let paths = temp_paths("reject-non-object");
        seed(&paths);
        let before: Vec<Vec<u8>> = [
            &paths.checkin_config,
            &paths.travel_config,
            &paths.rotate_config,
            &paths.rate_limit_config,
        ]
        .iter()
        .map(|path| read_bytes(path.as_path()))
        .collect();

        for payload in [json!("not-a-bundle"), json!([1, 2, 3]), json!(42)] {
            let error =
                import_settings_bundle_at(&paths, &payload, 1).expect_err("非法 JSON 必须被拒绝");
            assert!(error.contains("JSON 对象"), "{error}");
        }

        let after: Vec<Vec<u8>> = [
            &paths.checkin_config,
            &paths.travel_config,
            &paths.rotate_config,
            &paths.rate_limit_config,
        ]
        .iter()
        .map(|path| read_bytes(path.as_path()))
        .collect();
        assert_eq!(before, after, "被拒绝的导入不得改动任何配置文件");
    }

    #[test]
    fn import_rejects_unknown_key_without_touching_files() {
        let paths = temp_paths("reject-unknown-key");
        seed(&paths);
        let before = read_bytes(&paths.checkin_config);

        // 未知配置项（`update` 刻意不在白名单里：那份可能带代理口令）。
        let error = import_settings_bundle_at(
            &paths,
            &json!({ "version": 1, "configs": { "update": { "proxy": "http://example.com" } } }),
            1,
        )
        .expect_err("未知键必须被拒绝");
        assert!(error.contains("未知配置项 update"), "{error}");

        // 已知键里混入未知字段同样拒绝。
        let error = import_settings_bundle_at(
            &paths,
            &json!({ "version": 1, "configs": { "travel": { "enabled": true, "token": "x" } } }),
            1,
        )
        .expect_err("未知字段必须被拒绝");
        assert!(error.contains("token"), "{error}");

        assert_eq!(before, read_bytes(&paths.checkin_config));
    }

    #[test]
    fn import_rejects_partial_config_without_touching_files() {
        let paths = temp_paths("reject-partial");
        seed(&paths);
        let before: Vec<Vec<u8>> = [
            &paths.checkin_config,
            &paths.travel_config,
            &paths.rotate_config,
            &paths.rate_limit_config,
        ]
        .iter()
        .map(|path| read_bytes(path.as_path()))
        .collect();

        // 半截对象（只有 enabled）：整体拒绝，且不写任何一份（含合法的 travel）。
        let error = import_settings_bundle_at(
            &paths,
            &json!({
                "version": 1,
                "configs": {
                    "travel": { "enabled": true },
                    "checkin": { "enabled": true },
                }
            }),
            1,
        )
        .expect_err("半截对象必须被拒绝");
        assert!(error.contains("缺少字段"), "{error}");

        // 类型不符（`"true"` 字符串）：同样拒绝，不被静默降级。
        let error = import_settings_bundle_at(
            &paths,
            &json!({ "version": 1, "configs": { "travel": { "enabled": "true" } } }),
            1,
        )
        .expect_err("类型不符必须被拒绝");
        assert!(error.contains("类型不正确"), "{error}");

        let after: Vec<Vec<u8>> = [
            &paths.checkin_config,
            &paths.travel_config,
            &paths.rotate_config,
            &paths.rate_limit_config,
        ]
        .iter()
        .map(|path| read_bytes(path.as_path()))
        .collect();
        assert_eq!(before, after, "被拒绝的导入不得改动任何配置文件");
    }

    #[test]
    fn import_rejects_unsupported_version() {
        let paths = temp_paths("reject-version");
        let error = import_settings_bundle_at(
            &paths,
            &json!({ "version": 99, "configs": { "travel": { "enabled": true } } }),
            1,
        )
        .expect_err("未知版本必须被拒绝");
        assert!(error.contains("版本不受支持"), "{error}");
    }

    #[test]
    fn export_omits_unknown_fields_and_fills_defaults() {
        let paths = temp_paths("export-normalize");
        // 手工塞进额外字段的轮换配置：导出只保留白名单字段。
        std::fs::write(
            &paths.rotate_config,
            "{\"enabled\": true, \"check_interval_minutes\": 9, \"secret\": \"x\"}",
        )
        .expect("写入轮换配置");
        let bundle = export_settings_bundle_at(&paths, 1);
        let rotate = bundle["configs"]["rotate"].as_object().expect("轮换配置");
        assert_eq!(rotate.get("secret"), None);
        assert_eq!(rotate.get("check_interval_minutes"), Some(&json!(9)));
        // 缺失字段用默认值补齐，保证导出结果可以原样导回。
        assert_eq!(rotate.get("cooldown_minutes"), Some(&json!(120)));
    }
}

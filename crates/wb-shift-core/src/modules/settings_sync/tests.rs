//! 设置同步的合并与写入口径测试：全部在临时目录造数。

use super::*;
use serde_json::json;

struct Fixture {
    base: PathBuf,
    ctx: SettingsSyncContext,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let base =
            std::env::temp_dir().join(format!("wb-settings-sync-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let store = base.join(".workbuddy-shift");
        let cn = base.join(".workbuddy");
        let ai = base.join(".workbuddy-ai");
        std::fs::create_dir_all(&store).expect("建工具根");
        std::fs::create_dir_all(&cn).expect("建国内根");
        std::fs::create_dir_all(&ai).expect("建国际根");
        let accounts = vec![
            json!({ "id": "a1", "uid": "uid-a", "email": "a@example.com", "variant": "cn" }),
            json!({ "id": "b1", "uid": "uid-b", "email": "b@example.com", "variant": "cn" }),
            json!({ "id": "i1", "uid": "uid-i", "email": "i@example.com", "variant": "ai" }),
        ];
        Self {
            ctx: SettingsSyncContext {
                accounts,
                store_root: store,
                cn_root: cn,
                ai_root: ai,
            },
            base,
        }
    }

    fn account_dir(&self, uid: &str) -> PathBuf {
        self.ctx
            .cn_root
            .join("storage")
            .join(format!("user-{uid}-personal"))
    }

    fn write_prefs(&self, uid: &str, value: Value) {
        let dir = self.account_dir(uid).join("global");
        std::fs::create_dir_all(&dir).expect("建 global");
        std::fs::write(
            dir.join("preferences.json"),
            serde_json::to_string_pretty(&value).expect("序列化"),
        )
        .expect("写偏好");
    }

    fn write_scope(&self, uid: &str, scope: &str, value: Value) {
        let dir = self.account_dir(uid).join("scoped").join(scope);
        std::fs::create_dir_all(&dir).expect("建 scope");
        std::fs::write(
            dir.join("preferences.json"),
            serde_json::to_string_pretty(&value).expect("序列化"),
        )
        .expect("写 scope 偏好");
    }

    fn read_prefs(&self, uid: &str) -> Map<String, Value> {
        read_prefs(
            &self
                .account_dir(uid)
                .join("global")
                .join("preferences.json"),
        )
        .expect("读偏好")
    }

    /// 状态文件里的同步记录（最新在前）。
    fn history(&self) -> Vec<Value> {
        let path = self.ctx.store_root.join("settings_sync_state.json");
        let text = std::fs::read_to_string(path).expect("读状态文件");
        let value: Value = serde_json::from_str(&text).expect("解析状态文件");
        value["history"]
            .as_array()
            .cloned()
            .expect("history 必须是数组")
    }

    /// 某账号的备份目录名（升序）。
    fn backup_dirs(&self, uid: &str) -> Vec<String> {
        let dir = self
            .ctx
            .store_root
            .join("backups")
            .join("settings")
            .join(uid);
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        names
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn map(value: Value) -> Map<String, Value> {
    value.as_object().cloned().expect("对象")
}

#[test]
fn merge_overwrites_source_keys_and_keeps_target_only_keys() {
    let source =
        map(json!({ "theme": "dark", "appearanceTheme": "dark", "tipSoundStyle": "calm" }));
    let target = map(json!({ "theme": "light", "localTaskPermissionMode": "full-access" }));
    let outcome = merge_preferences(&source, &target, "uid-b", 1_700_000_000_000);

    assert_eq!(outcome.merged.get("theme"), Some(&json!("dark")));
    assert_eq!(
        outcome.merged.get("localTaskPermissionMode"),
        Some(&json!("full-access")),
        "目标独有键必须保留"
    );
    assert_eq!(outcome.changed.len(), 3);
}

#[test]
fn denylist_keys_are_skipped_and_reported() {
    let source = map(json!({
        "theme": "dark",
        "extensionListRefresh": { "sequence": 3 },
        "officeWindowGrowthNudge": { "shown": true },
        "loadingTipsDismissed": true,
    }));
    let outcome = merge_preferences(&source, &Map::new(), "uid-b", 1);
    assert!(!outcome.merged.contains_key("extensionListRefresh"));
    assert!(!outcome.merged.contains_key("loadingTipsDismissed"));
    assert_eq!(outcome.skipped.len(), 3);
}

#[test]
fn theme_selection_uid_and_revision_are_rewritten() {
    let source = map(json!({
        "appearance.themeSelection": {
            "resourceKey": "theme-tkbdx8",
            "uid": "uid-a",
            "source": "settings-select",
            "revision": 100,
        }
    }));
    let outcome = merge_preferences(&source, &Map::new(), "uid-b", 50);
    let selection = outcome.merged.get(THEME_SELECTION_KEY).expect("主题键");
    assert_eq!(selection["uid"], json!("uid-b"), "uid 必须改写成目标账号");
    assert_eq!(selection["resourceKey"], json!("theme-tkbdx8"));
    assert_eq!(selection["revision"], json!(101), "至少是源 revision + 1");

    // now 更大时取 now。
    let later = merge_preferences(&source, &Map::new(), "uid-b", 5_000);
    assert_eq!(later.merged[THEME_SELECTION_KEY]["revision"], json!(5_000));
}

#[test]
fn apply_writes_target_and_creates_backup_manifest() {
    let fixture = Fixture::new("apply");
    fixture.write_prefs(
        "uid-a",
        json!({ "theme": "dark", "appearanceTheme": "dark" }),
    );
    fixture.write_prefs(
        "uid-b",
        json!({ "theme": "light", "appearanceTheme": "light" }),
    );

    let result = apply(&fixture.ctx, Some("a1"), false).expect("同步成功");
    let target = &result["targets"][0];
    assert_eq!(target["status"], json!("synced"));
    let prefs = fixture.read_prefs("uid-b");
    assert_eq!(prefs.get("theme"), Some(&json!("dark")));

    let backup = PathBuf::from(target["backup"].as_str().expect("备份路径"));
    assert!(backup.join("manifest.json").is_file(), "必须有备份清单");
    assert!(
        backup.join("global").join("preferences.json").is_file(),
        "必须备份目标原文件"
    );
    let manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(backup.join("manifest.json")).expect("读清单"),
    )
    .expect("解析清单");
    assert_eq!(manifest["files"][0]["existed"], json!(true));
}

#[test]
fn backup_failure_writes_nothing() {
    let fixture = Fixture::new("backup-fail");
    fixture.write_prefs("uid-a", json!({ "theme": "dark" }));
    fixture.write_prefs("uid-b", json!({ "theme": "light" }));
    // 把 backups/settings 变成普通文件：创建备份目录必然失败。
    let blocker = fixture.ctx.store_root.join("backups");
    std::fs::create_dir_all(&blocker).expect("建 backups");
    std::fs::write(blocker.join("settings"), b"not-a-dir").expect("占位文件");

    let result = apply(&fixture.ctx, Some("a1"), false).expect("返回分项结果");
    let target = &result["targets"][0];
    assert_eq!(target["status"], json!("failed"));
    let prefs = fixture.read_prefs("uid-b");
    assert_eq!(
        prefs.get("theme"),
        Some(&json!("light")),
        "备份失败时不得写入目标"
    );
}

#[test]
fn target_without_directory_is_skipped() {
    let fixture = Fixture::new("no-dir");
    fixture.write_prefs("uid-a", json!({ "theme": "dark" }));
    // uid-b 没有目录（从未登录）。
    let result = apply(&fixture.ctx, Some("a1"), false).expect("返回分项结果");
    let skipped: Vec<&Value> = result["targets"]
        .as_array()
        .expect("数组")
        .iter()
        .filter(|target| target["status"] == json!("skipped"))
        .collect();
    assert!(!skipped.is_empty());
    assert!(
        skipped[0]["reason"]
            .as_str()
            .unwrap_or_default()
            .contains("目录不存在"),
        "{:?}",
        skipped[0]
    );
}

#[test]
fn sync_on_switch_respects_restart_and_variant() {
    let fixture = Fixture::new("switch");
    fixture.write_prefs("uid-a", json!({ "theme": "dark" }));
    fixture.write_prefs("uid-b", json!({ "theme": "light" }));

    let skipped = sync_on_switch(&fixture.ctx, WbVariant::Cn, "uid-a", "uid-b", false);
    assert_eq!(skipped["status"], json!("skipped"));
    assert!(skipped["reason"]
        .as_str()
        .unwrap_or_default()
        .contains("未重启"));
    assert_eq!(
        fixture.read_prefs("uid-b").get("theme"),
        Some(&json!("light"))
    );

    let cross = sync_on_switch(&fixture.ctx, WbVariant::Cn, "uid-a", "uid-i", true);
    assert_eq!(cross["status"], json!("rejected"));

    let synced = sync_on_switch(&fixture.ctx, WbVariant::Cn, "uid-a", "uid-b", true);
    assert_eq!(synced["status"], json!("synced"));
    assert_eq!(
        fixture.read_prefs("uid-b").get("theme"),
        Some(&json!("dark"))
    );
}

#[test]
fn scopes_only_sync_where_target_has_the_scope_directory() {
    let fixture = Fixture::new("scopes");
    fixture.write_prefs("uid-a", json!({ "theme": "dark" }));
    fixture.write_prefs("uid-b", json!({ "theme": "light" }));
    fixture.write_scope("uid-a", "c7389fe09fa872c0", json!({ "language": "en-US" }));
    fixture.write_scope("uid-b", "c7389fe09fa872c0", json!({ "language": "zh-CN" }));
    fixture.write_scope("uid-a", "0cf2de2dccc0c418", json!({ "language": "en-US" }));
    // 目标没有 0cf2de2dccc0c418 这个 scope。

    apply(&fixture.ctx, Some("a1"), false).expect("同步成功");
    let target_scope = fixture
        .account_dir("uid-b")
        .join("scoped")
        .join("c7389fe09fa872c0")
        .join("preferences.json");
    let value: Value =
        serde_json::from_str(&std::fs::read_to_string(target_scope).expect("读 scope"))
            .expect("解析");
    assert_eq!(value["language"], json!("en-US"));
    assert!(
        !fixture
            .account_dir("uid-b")
            .join("scoped")
            .join("0cf2de2dccc0c418")
            .exists(),
        "不得为目标创建它没有的 scope 目录"
    );
}

// ---------------------------------------------------------------------------
// 第三轮 P0-1：自动同步也留痕
// ---------------------------------------------------------------------------

#[test]
fn auto_sync_leaves_a_state_record() {
    let fixture = Fixture::new("auto-state");
    fixture.write_prefs("uid-a", json!({ "theme": "dark" }));
    fixture.write_prefs("uid-b", json!({ "theme": "light" }));

    let report = sync_on_switch(&fixture.ctx, WbVariant::Cn, "uid-a", "uid-b", true);
    assert_eq!(report["status"], json!("synced"));

    let history = fixture.history();
    assert_eq!(history.len(), 1, "自动同步必须留痕");
    let entry = &history[0];
    assert_eq!(entry["trigger"], json!("auto"));
    assert_eq!(entry["status"], json!("synced"));
    assert_eq!(entry["source"]["uid"], json!("uid-a"));
    assert_eq!(entry["targets"][0]["uid"], json!("uid-b"));
    assert_eq!(entry["targets"][0]["status"], json!("synced"));

    // status() 的 lastResult 取最新一条。
    let status = status(&fixture.ctx);
    assert_eq!(status["lastResult"]["trigger"], json!("auto"));
    assert_eq!(status["lastResult"]["at"], entry["at"]);
}

#[test]
fn skipped_auto_sync_also_leaves_a_record() {
    let fixture = Fixture::new("auto-skip-state");
    fixture.write_prefs("uid-a", json!({ "theme": "dark" }));
    fixture.write_prefs("uid-b", json!({ "theme": "light" }));

    // restart=false 只是跳过，但用户需要看到「为什么没同步」，所以同样留痕。
    let report = sync_on_switch(&fixture.ctx, WbVariant::Cn, "uid-a", "uid-b", false);
    assert_eq!(report["status"], json!("skipped"));

    let history = fixture.history();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0]["trigger"], json!("auto"));
    assert_eq!(history[0]["status"], json!("skipped"));
    assert!(history[0]["reason"]
        .as_str()
        .unwrap_or_default()
        .contains("未重启"));
}

// ---------------------------------------------------------------------------
// 第三轮 P0-2：预览只差 revision 时必须收敛
// ---------------------------------------------------------------------------

fn theme_selection(uid: &str, revision: i64) -> Value {
    json!({
        "resourceKey": "theme-light",
        "uid": uid,
        "source": "settings-select",
        "revision": revision,
    })
}

#[test]
fn cosmetic_theme_rewrite_is_not_counted_as_change() {
    let source = map(json!({ THEME_SELECTION_KEY: theme_selection("uid-a", 100) }));
    let target = map(json!({ THEME_SELECTION_KEY: theme_selection("uid-b", 500) }));

    let outcome = merge_preferences(&source, &target, "uid-b", 1_000);
    assert!(
        outcome.changed.is_empty(),
        "归属已经是目标账号、只有 revision 会更新的，不算变更：{:?}",
        outcome.changed
    );
    assert_eq!(outcome.cosmetic.len(), 1);
    assert_eq!(outcome.cosmetic[0]["key"], json!(THEME_SELECTION_KEY));
    assert!(outcome.theme_rewrite, "写入时仍会连带改写 revision");
    assert!(!outcome.has_real_changes());
}

#[test]
fn theme_uid_rewrite_is_still_a_real_change() {
    let source = map(json!({ THEME_SELECTION_KEY: theme_selection("uid-a", 100) }));
    // 目标主题还属于别的账号：归属改写是真实变更，不计入 cosmetic。
    let target = map(json!({ THEME_SELECTION_KEY: theme_selection("uid-z", 500) }));

    let outcome = merge_preferences(&source, &target, "uid-b", 1_000);
    assert_eq!(outcome.changed.len(), 1);
    assert_eq!(outcome.changed[0]["key"], json!(THEME_SELECTION_KEY));
    assert_eq!(
        outcome.changed[0]["to"]["uid"],
        json!("uid-b"),
        "必须改写到目标账号"
    );
    assert!(outcome.cosmetic.is_empty());
    assert!(outcome.theme_rewrite);
}

#[test]
fn preview_converges_after_first_sync() {
    let fixture = Fixture::new("converge");
    fixture.write_prefs(
        "uid-a",
        json!({ "appearanceTheme": "light", THEME_SELECTION_KEY: theme_selection("uid-a", 100) }),
    );
    fixture.write_prefs(
        "uid-b",
        json!({
            "appearanceTheme": "dark",
            THEME_SELECTION_KEY: json!({
                "resourceKey": "theme-dark",
                "uid": "uid-b",
                "source": "settings-select",
                "revision": 500,
            }),
        }),
    );

    let first = apply(&fixture.ctx, Some("a1"), false).expect("首次同步");
    assert_eq!(first["targets"][0]["status"], json!("synced"));
    assert_eq!(first["trigger"], json!("manual"));
    assert_eq!(first["counts"]["synced"], json!(1));

    // 第二次预览：除 revision 外全部一致 → 计 0 项，标记为「已一致」。
    let plan = preview(&fixture.ctx, Some("a1")).expect("预览");
    let target = &plan["targets"][0];
    assert_eq!(target["status"], json!("unchanged"));
    assert_eq!(target["changes"].as_array().map(Vec::len), Some(0));
    assert_eq!(target["cosmetic"].as_array().map(Vec::len), Some(1));

    // 第二次执行：无真实变更 → unchanged，且不再产生第二份备份。
    let second = apply(&fixture.ctx, Some("a1"), false).expect("二次同步");
    assert_eq!(second["targets"][0]["status"], json!("unchanged"));
    assert_eq!(second["status"], json!("unchanged"));
    assert_eq!(second["targets"][0]["backup"], Value::Null);
    assert_eq!(
        fixture.backup_dirs("uid-b").len(),
        1,
        "无真实变更不得写盘，也不得新建备份"
    );
}

#[test]
fn state_history_keeps_only_the_latest_twenty() {
    let fixture = Fixture::new("history-limit");
    fixture.write_prefs("uid-a", json!({ "theme": "dark" }));
    fixture.write_prefs("uid-b", json!({ "theme": "dark" }));

    for _ in 0..(STATE_HISTORY_LIMIT + 5) {
        apply(&fixture.ctx, Some("a1"), false).expect("同步");
    }
    assert_eq!(fixture.history().len(), STATE_HISTORY_LIMIT);
}

// ---------------------------------------------------------------------------
// 备份列举 / 还原
// ---------------------------------------------------------------------------

#[test]
fn list_and_restore_backup_round_trip() {
    let fixture = Fixture::new("restore");
    fixture.write_prefs(
        "uid-a",
        json!({ "theme": "dark", "appearanceTheme": "dark" }),
    );
    fixture.write_prefs(
        "uid-b",
        json!({ "theme": "light", "appearanceTheme": "light" }),
    );

    let result = apply(&fixture.ctx, Some("a1"), false).expect("同步成功");
    let stamp = fixture
        .backup_dirs("uid-b")
        .into_iter()
        .next()
        .expect("必须有备份目录");
    assert_eq!(
        fixture.read_prefs("uid-b").get("theme"),
        Some(&json!("dark"))
    );

    let listed = list_backups(&fixture.ctx, "uid-b").expect("列举备份");
    let entries = listed["entries"].as_array().expect("entries");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["stamp"], json!(stamp));
    assert_eq!(entries[0]["hasManifest"], json!(true));
    assert!(result["backupRoot"].is_string());

    let restored = restore_backup(&fixture.ctx, "uid-b", &stamp).expect("还原");
    assert_eq!(restored["uid"], json!("uid-b"));
    assert_eq!(
        fixture.read_prefs("uid-b").get("theme"),
        Some(&json!("light")),
        "还原后必须回到同步前的值"
    );
    // 还原前先把当前文件再备份一份。
    let pre = PathBuf::from(
        restored["preRestoreBackup"]
            .as_str()
            .expect("还原前备份路径"),
    );
    assert!(pre.join("manifest.json").is_file(), "必须留下还原前备份");
}

#[test]
fn restore_rejects_bad_uid_and_stamp() {
    let fixture = Fixture::new("restore-guard");
    fixture.write_prefs("uid-a", json!({ "theme": "dark" }));
    fixture.write_prefs("uid-b", json!({ "theme": "light" }));
    apply(&fixture.ctx, Some("a1"), false).expect("同步成功");
    let stamp = fixture
        .backup_dirs("uid-b")
        .into_iter()
        .next()
        .expect("备份目录");

    for bad in ["../uid-b", "uid/b", "", "a b", "uid.b"] {
        let error = restore_backup(&fixture.ctx, bad, &stamp).expect_err("uid 必须被拒绝");
        assert!(error.contains("uid"), "{bad} → {error}");
    }
    for bad in [
        "",
        "2025",
        "20250101",
        "20250101-1200",
        "../20250101-120000",
        "20251301-120000",
    ] {
        let error = restore_backup(&fixture.ctx, "uid-b", bad).expect_err("stamp 必须被拒绝");
        assert!(error.contains("时间戳"), "{bad} → {error}");
    }
    // 合法 stamp 但目录不存在。
    assert!(restore_backup(&fixture.ctx, "uid-b", "19990101-000000").is_err());
    // 目标设置未被改动。
    assert_eq!(
        fixture.read_prefs("uid-b").get("theme"),
        Some(&json!("dark"))
    );
}

#[test]
fn restore_removes_files_that_sync_created() {
    let fixture = Fixture::new("restore-remove");
    fixture.write_prefs("uid-a", json!({ "theme": "dark" }));
    // uid-b 有目录但没有 preferences.json：同步会新建该文件。
    std::fs::create_dir_all(fixture.account_dir("uid-b").join("global")).expect("建目录");

    apply(&fixture.ctx, Some("a1"), false).expect("同步成功");
    let target_file = fixture
        .account_dir("uid-b")
        .join("global")
        .join("preferences.json");
    assert!(target_file.is_file(), "同步应创建目标文件");

    let stamp = fixture
        .backup_dirs("uid-b")
        .into_iter()
        .next()
        .expect("备份目录");
    restore_backup(&fixture.ctx, "uid-b", &stamp).expect("还原");
    assert!(!target_file.is_file(), "同步新建的文件应在还原时删掉");
}

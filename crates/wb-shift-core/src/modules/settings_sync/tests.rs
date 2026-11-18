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
    assert_eq!(target["skipped"], json!(false));
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
    assert_eq!(target["skipped"], json!(true));
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
        .filter(|target| target["skipped"] == json!(true))
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

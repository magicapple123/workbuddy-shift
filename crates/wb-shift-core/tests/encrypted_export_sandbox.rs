//! 加密导出/导入在**真实沙箱家目录**下的端到端验收（issue #186）。
//!
//! # 为什么单开一个集成测试文件
//!
//! `import_accounts_encrypted_at` 里「账号库在哪」那一步（`account::load_accounts()` /
//! `account::save_accounts()`）没有参数可注入，只能靠 `WB_SHIFT_HOME` 重定向。该环境
//! 变量是**进程级**的，和 lib 里的其它用例跑在同一个测试二进制里会互相打架，因此单开
//! 本文件——**一个文件一条用例**，进程天然隔离，`set_var` 安全（与
//! `tests/token_stats_sandbox.rs` 同一套做法）。
//!
//! # 这条用例要证明什么
//!
//! 「错口令不写入」不能只断言"没报错"，也不能只断言内存里的 `Vec<Value>` 没变：合并
//! 结果只有走到最后一步才落盘，真正要钉住的是**磁盘上的 `accounts.json` 逐字节未变**。
//! 因此这里读的是文件字节，不是解析后的结构体。
//!
//! 同时还要挡住一个更隐蔽的失败模式：如果写入路径本身就是坏的（比如 `save_accounts`
//! 永远失败），「错口令后文件未变」会**假通过**。所以用例的最后一步用正确口令再导一次，
//! 断言账号库**确实变了**、被恢复的 token 确实在库里——证明前面那次"未变"是有意义的。

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use wb_shift_core::modules::account;
use wb_shift_core::modules::config::{accounts_file, HOME_ENV_VAR};
use wb_shift_core::modules::export_import::{
    export_accounts_encrypted_at, import_accounts_encrypted_at,
};

/// 导出时设置的口令（满足 8 位下限）。
const PASSWORD: &str = "sandbox-passphrase-2026";
/// 故意写错的口令（同样满足 8 位，失败原因只可能是"解密没过"）。
const WRONG_PASSWORD: &str = "wrong-passphrase-2026";
/// 边界内侧的短口令：绕过前端直接调 core / HTTP 时会用到的那一个。
const SHORT_PASSWORD: &str = "1234567";
/// 本机账号的 token（沙箱内自有，不是真实凭据）。
const LOCAL_TOKEN: &str = "local-token-C0FFEE";
/// 待恢复账号的 token。
const RESTORED_TOKEN: &str = "exported-token-BEEFCAFE";
/// 待恢复账号的邮箱。
const RESTORED_EMAIL: &str = "restored@example.com";
/// 固定导出时间，避免依赖系统时钟。
const CREATED_AT: i64 = 1_764_000_000_000;

/// 沙箱家目录：构造即设置 `WB_SHIFT_HOME`，析构还原并删目录。
struct Sandbox {
    root: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "workbuddy-shift-encrypted-sandbox-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("建沙箱根目录");
        std::env::set_var(HOME_ENV_VAR, &root);
        Self { root }
    }

    /// 加密导出文件落点（绝对路径 + `.json`，与 `validate_export_path` 的口径一致）。
    fn export_path(&self) -> String {
        self.root
            .join("workbuddy-shift-accounts-encrypted.json")
            .to_string_lossy()
            .into_owned()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        std::env::remove_var(HOME_ENV_VAR);
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// 账号库文件的当前字节（不存在时返回空 Vec，让"未变"断言仍然可比）。
fn accounts_bytes() -> Vec<u8> {
    std::fs::read(accounts_file()).unwrap_or_default()
}

/// 从账号库里按 uid 取一条（断言落盘内容用）。
fn find_by_uid<'a>(accounts: &'a [Value], uid: &str) -> Option<&'a Value> {
    accounts
        .iter()
        .find(|account| account.get("uid").and_then(Value::as_str) == Some(uid))
}

/// 把账号库**手动**写成紧凑 JSON（不给尾随换行、不缩进）。
///
/// 为什么不用 `account::save_accounts`：生产写入是 `to_string_pretty`。如果基线文件也
/// 是紧凑格式，那么"某处偷偷重新保存了一遍但内容没变"也会让文件字节发生变化——反过来，
/// 「逐字节未变」才有意义：它同时证明**没有发生任何写入**，而不只是"写回了同样的内容"。
fn write_accounts_compact(accounts: &[Value]) {
    let path = accounts_file();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("建沙箱 store 目录");
    }
    let content = serde_json::to_string(accounts).expect("序列化紧凑账号库");
    std::fs::write(&path, content).expect("写紧凑账号库");
}

#[test]
fn wrong_password_import_leaves_accounts_file_byte_identical() {
    let sandbox = Sandbox::new();
    let export_path = sandbox.export_path();

    // 1) 沙箱账号库：先有两条账号（本机号 + 待恢复号），模拟「导出时的机器」。
    let local = json!({
        "id": "local-1",
        "uid": "uid-local",
        "nickname": "本机账号",
        "email": "local@example.com",
        "access_token": LOCAL_TOKEN,
        "variant": "cn",
    });
    let restorable = json!({
        "id": "acc-2",
        "uid": "uid-restored",
        "nickname": "要恢复的账号",
        "email": RESTORED_EMAIL,
        "access_token": RESTORED_TOKEN,
        "variant": "ai",
    });
    write_accounts_compact(&[local.clone(), restorable.clone()]);

    // 2) 走**生产路径**导出：先 load_accounts() 读出整份，再加密写盘。
    let loaded = account::load_accounts();
    assert_eq!(loaded.len(), 2, "沙箱账号库应能读回两条: {loaded:?}");
    export_accounts_encrypted_at(&export_path, PASSWORD, &loaded, CREATED_AT)
        .expect("加密导出应成功");
    assert!(
        Path::new(&export_path).is_file(),
        "导出文件应落在沙箱里：{export_path}"
    );

    // 3) 账号库退回「只有本机号」（模拟换机后的空库），记下逐字节快照。
    //    基线用紧凑格式写，任何一次 `save_accounts`（pretty）都会让字节变掉。
    write_accounts_compact(&[local]);
    let before = accounts_bytes();
    assert!(!before.is_empty(), "重置后账号库不应为空文件");
    assert!(
        !String::from_utf8_lossy(&before).contains("\n  "),
        "基线必须是紧凑格式（否则「逐字节未变」无法区分「没写」与「写了同样的内容」）"
    );

    // 4) 错口令：必须报错，且磁盘上的账号库**逐字节未变**。
    let error =
        import_accounts_encrypted_at(&export_path, WRONG_PASSWORD).expect_err("错口令导入必须失败");
    assert!(
        error.contains("密码错误") || error.contains("损坏"),
        "错误应是可读文案：{error}"
    );
    assert!(
        !error.contains(WRONG_PASSWORD),
        "错误文案不得回显口令：{error}"
    );
    assert!(
        !error.contains(RESTORED_TOKEN),
        "错误文案不得包含明文片段：{error}"
    );
    assert_eq!(
        accounts_bytes(),
        before,
        "错口令导入后账号库必须逐字节未变（不是「没报错」就算通过）"
    );

    // 5) 边界内侧的 7 位口令（绕过前端直接调 core / HTTP 时的形状）同样被拒，
    //    账号库依旧逐字节未变——口令策略的裁决点在 core，不在前端。
    let error = import_accounts_encrypted_at(&export_path, SHORT_PASSWORD)
        .expect_err("7 位口令必须被 core 拒绝");
    assert!(
        error.contains("至少 8 位"),
        "错误文案应明确 8 位下限：{error}"
    );
    assert!(
        !error.contains(SHORT_PASSWORD),
        "错误文案不得回显口令：{error}"
    );
    assert_eq!(accounts_bytes(), before, "短口令被拒后账号库必须逐字节未变");

    // 6) 正确口令：证明第 4/5 步的「未变」不是因为写入路径根本不通。
    //
    // 导出文件里是两条记录：本机号（沙箱账号库里还有，按 uid 覆盖）+ 待恢复号（新增）。
    let result = import_accounts_encrypted_at(&export_path, PASSWORD).expect("正确口令应导入成功");
    assert_eq!(
        result.imported, 2,
        "两条记录都应合入（一覆盖一新增）: {result:?}"
    );
    assert_eq!(result.overwritten, 1, "本机号应按 uid 被覆盖: {result:?}");
    assert_eq!(result.skipped, 0, "两条记录都该有凭据: {result:?}");

    let after = account::load_accounts();
    assert_eq!(after.len(), 2, "恢复后账号库应有两条: {after:?}");
    let restored = find_by_uid(&after, "uid-restored").expect("被恢复的账号应在账号库里");
    assert_eq!(
        restored.get("access_token"),
        Some(&json!(RESTORED_TOKEN)),
        "恢复的记录必须带着完整 token"
    );
    assert_ne!(
        accounts_bytes(),
        before,
        "正确口令必须真的写回账号库（否则第 4 步的「未变」是假通过）"
    );
}

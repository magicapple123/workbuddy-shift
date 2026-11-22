// 加密导出/导入文件桥接（`encrypted.rs`）的用例：往返、错口令不写账号、口令策略、
// 目标文件已存在、缺失/格式不符的文件。由 `mod.rs` 的 `mod tests` 通过 `include!`
// 引入，因此只能用普通注释而不是 `//!`（include 位置不在模块开头）。
//
// 这些用例同时钉住两条"不能悄悄退化"的性质：
//   1. 落盘的必须是信封，不是明文账号数组（读文件直接看不到 token）；
//   2. 任何失败路径都不写账号库（错口令 / 短口令只报错，不改一个字节）。

/// 桥接层测试口令：满足 8 位下限，且与 `crypto.rs` 用例的口令不同——
/// 这样"错误文案里不得包含口令"的断言不会因为两处共用同一个字符串而失真。
const BRIDGE_PASSWORD: &str = "bridge-passphrase-2026";

/// 临时目录里的唯一文件路径（每次调用独立，避免用例之间互相覆盖）。
fn bridge_temp_path(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("wb-encrypted-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("建临时目录");
    dir.join(format!("{name}-{}.json", uuid::Uuid::new_v4().simple()))
}

/// 与真实导出同形状的账号夹具（复用 `crypto.rs` 的 `sample_payload`，避免两套样例漂移）。
fn bridge_sample_accounts() -> Vec<Value> {
    serde_json::from_slice(&sample_payload()).expect("示例 payload 应是账号数组")
}

#[test]
fn encrypted_export_then_import_restores_same_accounts() {
    let accounts = bridge_sample_accounts();
    let path = bridge_temp_path("roundtrip");
    export_accounts_encrypted_at(
        &path.to_string_lossy(),
        BRIDGE_PASSWORD,
        &accounts,
        TEST_CREATED_AT,
    )
    .expect("加密导出应成功");

    // 落盘的是信封：token / 邮箱明文都不能直接读到。
    let bytes = std::fs::read(&path).expect("读回导出文件");
    let text = String::from_utf8(bytes.clone()).expect("信封应是 UTF-8 JSON");
    assert!(text.contains("wb-shift-export"), "导出文件应是本工具的信封");
    assert!(!text.contains(TOKEN_MARKER), "加密导出文件不得含 token 明文");
    assert!(!text.contains("user@example.com"), "不得含邮箱明文");
    assert!(!text.contains(BRIDGE_PASSWORD), "不得含口令");

    // 导入侧：同一口令解密 → 走既有合并语义 → 得到同样的账号集。
    let mut imported: Vec<Value> = Vec::new();
    let result =
        merge_encrypted_bytes(&bytes, BRIDGE_PASSWORD, &mut imported).expect("解密合并应成功");
    assert_eq!(result.imported, accounts.len(), "全部记录都应合入");
    assert_eq!(result.skipped, 0);
    assert_eq!(result.overwritten, 0);
    assert_eq!(imported.len(), accounts.len());
    assert_eq!(
        imported[0]["access_token"], accounts[0]["access_token"],
        "往返必须保留 access_token"
    );
    assert_eq!(imported[1]["uid"], accounts[1]["uid"]);
    assert_eq!(imported[1]["email"], accounts[1]["email"]);

    let _ = std::fs::remove_file(&path);
}

#[test]
fn encrypted_import_with_wrong_password_writes_nothing() {
    let accounts = bridge_sample_accounts();
    let path = bridge_temp_path("wrong-password");
    export_accounts_encrypted_at(
        &path.to_string_lossy(),
        BRIDGE_PASSWORD,
        &accounts,
        TEST_CREATED_AT,
    )
    .expect("加密导出应成功");
    let bytes = std::fs::read(&path).expect("读回导出文件");

    // 账号库里已经有一份本地账号：错口令必须让它一个字节都不变。
    let mut existing = vec![record("local", Some("uid-local"), "本地号", None, true)];
    let before = existing.clone();
    let error = merge_encrypted_bytes(&bytes, "wrong-password-entirely", &mut existing)
        .expect_err("错口令必须失败");
    assert!(
        error.contains("密码错误") || error.contains("损坏"),
        "错误应是可读文案：{error}"
    );
    assert!(!error.contains(BRIDGE_PASSWORD), "错误文案不得包含口令：{error}");
    assert!(!error.contains(TOKEN_MARKER), "错误文案不得包含明文片段：{error}");
    assert_eq!(existing, before, "错口令不得写入任何账号");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn encrypted_password_shorter_than_eight_chars_is_rejected() {
    let accounts = bridge_sample_accounts();
    let path = bridge_temp_path("short-password");

    for password in ["", "1234567"] {
        let error = export_accounts_encrypted_at(
            &path.to_string_lossy(),
            password,
            &accounts,
            TEST_CREATED_AT,
        )
        .expect_err("不足 8 位的口令必须拒绝");
        assert!(error.contains("至少 8 位"), "错误文案应明确 8 位下限：{error}");
        if !password.is_empty() {
            assert!(!error.contains(password), "错误文案不得回显口令：{error}");
        }
        assert!(!path.exists(), "被拒绝的导出不得留下文件（口令={password:?}）");
    }

    // 导入侧同一套策略：空口令在打开文件之前就被拒绝，同样报 8 位下限。
    let error =
        import_accounts_encrypted_at(&path.to_string_lossy(), "").expect_err("空口令必须拒绝");
    assert!(error.contains("至少 8 位"), "错误文案应明确 8 位下限：{error}");

    // 7 位（边界内侧）同样拒绝，8 位放行：把下限钉在"字符数 >= 8"而不是"> 8"。
    assert!(validate_password("1234567").is_err());
    assert!(validate_password("12345678").is_ok());
    // 中文口令按字符计：4 个汉字 = 4 位，仍然拒绝（不按字节算成 12 位）。
    assert!(validate_password("口令口令口令口令").is_ok());
    assert!(validate_password("口令口令").is_err());
}

#[test]
fn encrypted_export_replaces_existing_file() {
    let accounts = bridge_sample_accounts();
    let path = bridge_temp_path("overwrite");
    std::fs::write(&path, br#"{"stale":true}"#).expect("写占位文件");

    export_accounts_encrypted_at(
        &path.to_string_lossy(),
        BRIDGE_PASSWORD,
        &accounts,
        TEST_CREATED_AT,
    )
    .expect("覆盖已有文件应成功");

    let bytes = std::fs::read(&path).expect("读回导出文件");
    let mut imported: Vec<Value> = Vec::new();
    merge_encrypted_bytes(&bytes, BRIDGE_PASSWORD, &mut imported)
        .expect("覆盖后的文件应能用同一口令解开");
    assert_eq!(imported.len(), accounts.len(), "覆盖后的文件只含本次导出内容");

    // 原子写不残留临时文件（`atomic_write_bytes` 的 `<name>.tmp-*`）。
    let leftovers: Vec<String> = std::fs::read_dir(path.parent().expect("父目录存在"))
        .expect("列目录")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".tmp-"))
        .collect();
    assert!(leftovers.is_empty(), "不应残留临时文件：{leftovers:?}");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn encrypted_import_rejects_missing_and_non_envelope_files() {
    // 文件不存在：可读错误，而不是"导入了 0 个账号"。
    let missing = bridge_temp_path("missing");
    let error = import_accounts_encrypted_at(&missing.to_string_lossy(), BRIDGE_PASSWORD)
        .expect_err("文件不存在必须报错");
    assert!(error.contains("不存在"), "错误文案应可读：{error}");

    // 普通（未加密的）备份文件：明确拒绝，别再退回明文导入路径。
    let plain = bridge_temp_path("plain-json");
    std::fs::write(
        &plain,
        serde_json::to_vec(&json!([record("a1", Some("u1"), "甲", None, true)]))
            .expect("序列化明文夹具"),
    )
    .expect("写明文文件");
    let error = import_accounts_encrypted_at(&plain.to_string_lossy(), BRIDGE_PASSWORD)
        .expect_err("非信封文件必须报错");
    assert!(
        error.contains("不是合法的导出文件") || error.contains("不是本工具"),
        "错误文案应可读：{error}"
    );
    let _ = std::fs::remove_file(&plain);

    // 空路径：可读错误，不是 panic。
    let error = import_accounts_encrypted_at("   ", BRIDGE_PASSWORD).expect_err("空路径必须报错");
    assert!(error.contains("缺少导入文件路径"), "{error}");
}

#[test]
fn encrypted_export_rejects_bad_path_and_empty_accounts() {
    let accounts = bridge_sample_accounts();
    let dir = std::env::temp_dir();

    // 相对路径与非法扩展名：沿用明文导出的白名单（绝对路径 + .json）。
    let relative = "relative/encrypted.json";
    let error = export_accounts_encrypted_at(relative, BRIDGE_PASSWORD, &accounts, TEST_CREATED_AT)
        .expect_err("相对路径必须拒绝");
    assert!(error.contains("绝对路径"), "{error}");

    let not_json = dir.join("encrypted.txt").to_string_lossy().into_owned();
    let error =
        export_accounts_encrypted_at(&not_json, BRIDGE_PASSWORD, &accounts, TEST_CREATED_AT)
            .expect_err("非 .json 扩展名必须拒绝");
    assert!(error.contains(".json"), "{error}");

    // 没有任何账号：与 `select_export_records` 同一句提示，不留空文件。
    let path = bridge_temp_path("no-accounts");
    let error = export_accounts_encrypted_at(
        &path.to_string_lossy(),
        BRIDGE_PASSWORD,
        &[],
        TEST_CREATED_AT,
    )
    .expect_err("空账号集必须拒绝");
    assert!(error.contains("请先选择"), "{error}");
    assert!(!path.exists());
}

/// 落盘的导出文件里**任何形式的凭据都不能出现**：明文、base64 都不能。
///
/// 三种"看起来加密了、其实没有"的写法各挡一次：
/// 1. 忘了加密，直接把明文 JSON 写进文件；
/// 2. 只套了一层 base64（`data` 字段是明文的 base64）；
/// 3. `data` 字段解 base64 后仍是明文（前两种的合体）。
///
/// base64 一律走完全限定写法（`base64::Engine::encode`），不依赖 `crypto.rs` 里
/// 同模块 `use` 进来的 `BASE64` 常量——`include!` 的落地顺序变了也不会失效。
#[test]
fn encrypted_export_file_contains_no_plaintext_or_base64_credentials() {
    let accounts = bridge_sample_accounts();
    let path = bridge_temp_path("no-credentials");
    export_accounts_encrypted_at(
        &path.to_string_lossy(),
        BRIDGE_PASSWORD,
        &accounts,
        TEST_CREATED_AT,
    )
    .expect("加密导出应成功");

    let bytes = std::fs::read(&path).expect("读回导出文件");
    let text = String::from_utf8(bytes.clone()).expect("信封应是 UTF-8 JSON");

    // 1) 明文形式：两条账号的 token 与邮箱、以及口令本身。
    for needle in [
        TOKEN_MARKER,
        "wb-secret-token-CAFEBABE",
        "user@example.com",
        "other@example.com",
        BRIDGE_PASSWORD,
    ] {
        assert!(
            !text.contains(needle),
            "导出文件不得出现明文 `{needle}`：{}",
            text.chars().take(200).collect::<String>()
        );
    }

    // 2) base64 形式：整份明文 payload 的 base64 绝不能出现在文件里。
    let plain = serde_json::to_vec_pretty(&accounts).expect("序列化明文 payload");
    let plain_b64 = base64_encode(&plain);
    assert!(
        !text.contains(&plain_b64),
        "导出文件不得出现 base64(明文 payload)：这不叫加密"
    );

    // 3) `data` 解 base64 后必须是密文：既不是明文 payload，也不含凭据片段。
    let envelope: Value = serde_json::from_slice(&bytes).expect("信封应是 JSON");
    let ciphertext = base64_decode(envelope["data"].as_str().unwrap_or_default());
    assert_ne!(
        ciphertext, plain,
        "`data` 字段解 base64 后就是明文：加密没有生效"
    );
    let raw = String::from_utf8_lossy(&ciphertext);
    for needle in [TOKEN_MARKER, "user@example.com"] {
        assert!(
            !raw.contains(needle),
            "`data` 解 base64 后仍含明文 `{needle}`：加密没有生效"
        );
    }

    let _ = std::fs::remove_file(&path);
}

/// `base64` 标准字母表编码（单独包一层，避免用例里到处写完全限定路径）。
fn base64_encode(bytes: &[u8]) -> String {
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes)
}

/// 同上，解码；非法 base64 直接判失败（信封的 `data` 必须是合法 base64）。
fn base64_decode(text: &str) -> Vec<u8> {
    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, text)
        .expect("信封的 data 应是合法 base64")
}

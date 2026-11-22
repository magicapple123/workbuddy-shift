// 加密导出信封（`crypto.rs`）的用例：往返、错密码、篡改、格式拒绝、迭代上限、
// 无明文泄漏、随机性。由 `mod.rs` 的 `mod tests` 通过 `include!` 引入，
// 因此只能用普通注释而不是 `//!`（include 位置不在模块开头）。

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde_json::{json, Value};

/// 带明显特征的 token：断言导出文件里不含它（明文或 base64 形式）。
const TOKEN_MARKER: &str = "wb-secret-token-DEADBEEF";
/// 测试口令（只用于本文件断言"错误文案不含口令"）。
const TEST_PASSWORD: &str = "correct horse battery staple";
/// 固定导出时间，避免依赖系统时钟。
const TEST_CREATED_AT: i64 = 1_764_000_000_000;

/// 构造一段与真实导出同形状的 payload（账号数组，含 token）。
fn sample_payload() -> Vec<u8> {
    let accounts = json!([
        {
            "id": "acc-1",
            "uid": "uid-1",
            "nickname": "示例账号",
            "email": "user@example.com",
            "access_token": TOKEN_MARKER,
            "createdAt": 1,
        },
        {
            "id": "acc-2",
            "uid": "uid-2",
            "nickname": "第二个账号",
            "email": "other@example.com",
            "access_token": "wb-secret-token-CAFEBABE",
            "createdAt": 2,
        }
    ]);
    serde_json::to_vec(&accounts).expect("序列化示例 payload")
}

/// 加密一次并把信封解析成 `Value`，供负向用例改字段。
fn envelope_value(plain: &[u8], password: &str) -> Value {
    let bytes = encrypt_export(plain, password, TEST_CREATED_AT).expect("加密应成功");
    serde_json::from_slice(&bytes).expect("信封应是 JSON")
}

/// 改完字段后重新序列化成字节，模拟"用户手上被改过的文件"。
fn rewritten(mut envelope: Value, mutate: impl FnOnce(&mut Value)) -> Vec<u8> {
    mutate(&mut envelope);
    serde_json::to_vec(&envelope).expect("重新序列化信封")
}

/// 翻转密文的第一个字节（GCM 应当拒绝）。
fn flip_ciphertext_byte(envelope: &mut Value) {
    let encoded = envelope["data"].as_str().expect("data 应是字符串");
    let mut raw = BASE64.decode(encoded).expect("密文应是合法 base64");
    raw[0] ^= 0x01;
    envelope["data"] = json!(BASE64.encode(raw));
}

#[test]
fn roundtrip_recovers_plaintext_bytes() {
    let plain = sample_payload();
    let bytes = encrypt_export(&plain, TEST_PASSWORD, TEST_CREATED_AT).expect("加密应成功");

    // 信封字段与取值：读取端、文档与手工排查都依赖这套形状。
    let value: Value = serde_json::from_slice(&bytes).expect("信封应是 JSON");
    assert_eq!(value["format"], json!("wb-shift-export"));
    assert_eq!(value["version"], json!(1));
    assert_eq!(value["kdf"]["algo"], json!("pbkdf2-sha256"));
    assert_eq!(value["kdf"]["iterations"], json!(600_000));
    assert_eq!(value["cipher"]["algo"], json!("aes-256-gcm"));
    assert_eq!(value["createdAt"], json!(TEST_CREATED_AT));
    let salt = BASE64
        .decode(value["kdf"]["salt"].as_str().expect("salt 应是字符串"))
        .expect("salt 应是 base64");
    assert_eq!(salt.len(), 16, "盐必须是 16 字节");
    let nonce = BASE64
        .decode(value["cipher"]["nonce"].as_str().expect("nonce 应是字符串"))
        .expect("nonce 应是 base64");
    assert_eq!(nonce.len(), 12, "nonce 必须是 12 字节");

    let restored = decrypt_export(&bytes, TEST_PASSWORD).expect("解密应成功");
    assert_eq!(restored, plain, "往返必须得到原字节");
}

#[test]
fn wrong_password_fails_without_returning_plaintext() {
    let plain = sample_payload();
    let bytes = encrypt_export(&plain, TEST_PASSWORD, TEST_CREATED_AT).expect("加密应成功");

    let error = decrypt_export(&bytes, "definitely-not-the-password").expect_err("错密码必须失败");
    assert!(error.contains("密码错误"), "错误应是可读文案：{error}");
    assert!(
        !error.contains(TEST_PASSWORD),
        "错误文案不得包含密码：{error}"
    );
    assert!(
        !error.contains(TOKEN_MARKER),
        "错误文案不得包含明文片段：{error}"
    );
}

#[test]
fn tampered_ciphertext_fails_gcm_check() {
    let plain = sample_payload();
    let envelope = envelope_value(&plain, TEST_PASSWORD);
    let tampered = rewritten(envelope, flip_ciphertext_byte);

    let error = decrypt_export(&tampered, TEST_PASSWORD).expect_err("篡改必须被 GCM 拒绝");
    assert!(
        error.contains("密码错误") || error.contains("损坏"),
        "错误应是可读文案：{error}"
    );
}

#[test]
fn rejects_unknown_format_version_and_algo() {
    let plain = sample_payload();
    let base = envelope_value(&plain, TEST_PASSWORD);

    let wrong_format = rewritten(base.clone(), |v| v["format"] = json!("other-export"));
    let error = decrypt_export(&wrong_format, TEST_PASSWORD).expect_err("format 不符必须拒绝");
    assert!(error.contains("不是本工具"), "{error}");

    let wrong_version = rewritten(base.clone(), |v| v["version"] = json!(2));
    let error = decrypt_export(&wrong_version, TEST_PASSWORD).expect_err("版本不符必须拒绝");
    assert!(error.contains("版本"), "{error}");

    let wrong_kdf = rewritten(base.clone(), |v| v["kdf"]["algo"] = json!("scrypt"));
    let error = decrypt_export(&wrong_kdf, TEST_PASSWORD).expect_err("kdf 不符必须拒绝");
    assert!(error.contains("密钥派生"), "{error}");

    let wrong_cipher = rewritten(base.clone(), |v| v["cipher"]["algo"] = json!("aes-128-gcm"));
    let error = decrypt_export(&wrong_cipher, TEST_PASSWORD).expect_err("cipher 不符必须拒绝");
    assert!(error.contains("加密算法"), "{error}");

    // 未知字段同样拒绝：不给"多塞字段还能读"留模糊边界。
    let unknown_field = rewritten(base, |v| v["extra"] = json!(1));
    assert!(decrypt_export(&unknown_field, TEST_PASSWORD).is_err());
}

#[test]
fn rejects_iteration_count_above_limit() {
    let base = envelope_value(&sample_payload(), TEST_PASSWORD);

    let too_many = rewritten(base.clone(), |v| {
        v["kdf"]["iterations"] = json!(5_000_001_u32)
    });
    let error = decrypt_export(&too_many, TEST_PASSWORD).expect_err("超上限必须拒绝");
    assert!(error.contains("上限"), "{error}");

    let zero = rewritten(base, |v| v["kdf"]["iterations"] = json!(0));
    assert!(
        decrypt_export(&zero, TEST_PASSWORD).is_err(),
        "0 次必须拒绝"
    );
}

#[test]
fn iteration_count_is_read_from_envelope() {
    let base = envelope_value(&sample_payload(), TEST_PASSWORD);

    // 次数被改小 → 派生的密钥不同 → GCM 认证失败。这证明读取端用的是信封里的次数，
    // 而不是硬编码常量（否则这里会"解密成功"）。
    let lowered = rewritten(base, |v| v["kdf"]["iterations"] = json!(10_000));
    let error = decrypt_export(&lowered, TEST_PASSWORD).expect_err("次数被改必须失败");
    assert!(
        error.contains("密码错误") || error.contains("损坏"),
        "错误应是可读文案：{error}"
    );
}

#[test]
fn export_file_contains_no_token_plaintext() {
    let plain = sample_payload();
    // 先确认断言对象本身有效：明文里确实有该特征串。
    assert!(String::from_utf8_lossy(&plain).contains(TOKEN_MARKER));

    let bytes = encrypt_export(&plain, TEST_PASSWORD, TEST_CREATED_AT).expect("加密应成功");
    let text = String::from_utf8(bytes.clone()).expect("信封应是 UTF-8 JSON");
    assert!(!text.contains(TOKEN_MARKER), "信封不得包含 token 明文");
    let encoded_marker = BASE64.encode(TOKEN_MARKER.as_bytes());
    assert!(
        !text.contains(&encoded_marker),
        "信封也不得包含 token 的 base64"
    );
    assert!(!text.contains("user@example.com"), "信封不得包含邮箱明文");
    assert!(!text.contains(TEST_PASSWORD), "信封不得包含口令");
}

#[test]
fn same_plaintext_produces_different_ciphertext() {
    let plain = sample_payload();
    let first = envelope_value(&plain, TEST_PASSWORD);
    let second = envelope_value(&plain, TEST_PASSWORD);

    assert_ne!(
        first["kdf"]["salt"], second["kdf"]["salt"],
        "盐必须每次随机"
    );
    assert_ne!(
        first["cipher"]["nonce"], second["cipher"]["nonce"],
        "nonce 必须每次随机"
    );
    assert_ne!(
        first["data"], second["data"],
        "相同明文两次导出必须不同密文"
    );
}

#[test]
fn rejects_empty_truncated_and_malformed_files() {
    assert!(
        decrypt_export(b"", TEST_PASSWORD).is_err(),
        "空文件必须拒绝"
    );
    assert!(
        decrypt_export(b"not json at all", TEST_PASSWORD).is_err(),
        "非 JSON 必须拒绝"
    );

    let bytes =
        encrypt_export(&sample_payload(), TEST_PASSWORD, TEST_CREATED_AT).expect("加密应成功");

    // 截断：JSON 不完整。
    let truncated = &bytes[..bytes.len() / 2];
    assert!(
        decrypt_export(truncated, TEST_PASSWORD).is_err(),
        "截断文件必须拒绝"
    );

    // 密文短于 GCM 标签：合法 base64 但长度不足，必须报错而不是 panic。
    let short_data = rewritten(envelope_value(&sample_payload(), TEST_PASSWORD), |v| {
        let raw = BASE64
            .decode(v["data"].as_str().expect("data 应是字符串"))
            .expect("密文应是 base64");
        v["data"] = json!(BASE64.encode(&raw[..4]));
    });
    let error = decrypt_export(&short_data, TEST_PASSWORD).expect_err("短密文必须拒绝");
    assert!(error.contains("损坏"), "{error}");

    // nonce 长度不对：`Nonce::from_slice` 对非 12 字节会 panic，必须在调用前拦住。
    let bad_nonce = rewritten(envelope_value(&sample_payload(), TEST_PASSWORD), |v| {
        v["cipher"]["nonce"] = json!(BASE64.encode([0u8; 8]))
    });
    assert!(
        decrypt_export(&bad_nonce, TEST_PASSWORD).is_err(),
        "nonce 长度不符必须拒绝"
    );

    // 盐长度不对：同样先校验再派生。
    let bad_salt = rewritten(envelope_value(&sample_payload(), TEST_PASSWORD), |v| {
        v["kdf"]["salt"] = json!(BASE64.encode([0u8; 4]))
    });
    assert!(
        decrypt_export(&bad_salt, TEST_PASSWORD).is_err(),
        "盐长度不符必须拒绝"
    );

    // 空口令：导出与导入都拒绝（空口令等于不加密）。
    assert!(encrypt_export(b"[]", "", TEST_CREATED_AT).is_err());
    assert!(decrypt_export(&bytes, "").is_err());
}

/// 「密码错」与「文件被篡改」必须给出**逐字相同**的错误文案。
///
/// 两者在实现里共用同一个 GCM 失败分支；这条断言把它固定住，避免将来有人为了「体验」
/// 把两种原因分开报——那等于告诉离线爆破者「这个口令是对的、只是文件坏了」。
/// 同时断言文案里没有口令 / 明文 / 密文片段。
#[test]
fn wrong_password_and_tampered_file_are_indistinguishable() {
    let plain = sample_payload();
    // 只加密一次：PBKDF2 是 600000 次迭代，多用几次会把这条用例拖到秒级。
    let base = envelope_value(&plain, TEST_PASSWORD);
    let bytes = serde_json::to_vec(&base).expect("重新序列化信封");

    let wrong_password = decrypt_export(&bytes, "wrong-password").expect_err("错密码必须失败");
    let tampered = decrypt_export(&rewritten(base.clone(), flip_ciphertext_byte), TEST_PASSWORD)
        .expect_err("篡改必须失败");
    // 盐 / nonce 被改也落同一分支（同一把钥匙的错误都由 GCM 认证失败体现）。
    let nonce_changed = decrypt_export(
        &rewritten(base.clone(), |v| {
            v["cipher"]["nonce"] = json!(BASE64.encode([7u8; 12]))
        }),
        TEST_PASSWORD,
    )
    .expect_err("nonce 被改必须失败");

    assert_eq!(
        wrong_password, tampered,
        "错密码与篡改必须给出同一句文案：{wrong_password} / {tampered}"
    );
    assert_eq!(wrong_password, nonce_changed, "nonce 被改同样不得区分");
    // 文案本身不含任何秘密或密文片段。
    for secret in [TEST_PASSWORD, TOKEN_MARKER, "wb-secret-token-CAFEBABE"] {
        assert!(!wrong_password.contains(secret), "{wrong_password}");
    }
    let ciphertext = base["data"].as_str().expect("data 应是字符串");
    assert!(
        !wrong_password.contains(&ciphertext[..8]),
        "文案不得回显密文：{wrong_password}"
    );
}

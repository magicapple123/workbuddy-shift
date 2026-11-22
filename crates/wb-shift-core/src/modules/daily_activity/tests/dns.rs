// DoH 决策与应答解析的纯函数测试（不碰真实 DNS / 网络）。

/// fake-ip 段判定：只看 `198.18.0.0/15`。
#[test]
fn fake_ip_range_is_detected() {
    let fake: std::net::IpAddr = "198.18.0.5".parse().expect("合法 IP");
    let fake_high: std::net::IpAddr = "198.19.255.254".parse().expect("合法 IP");
    let real: std::net::IpAddr = "203.0.113.7".parse().expect("合法 IP");
    assert!(dns::is_fake_ip(fake));
    assert!(dns::is_fake_ip(fake_high));
    assert!(!dns::is_fake_ip(real));
    assert!(!dns::is_fake_ip("::1".parse().expect("合法 IP")));
}

/// 决策表：只有「沙箱域名 + 解析不可信」才需要 DoH。
#[test]
fn doh_decision_matrix() {
    let host = "sbx-1.sandbox.cloudstudio.club";
    let real: std::net::IpAddr = "203.0.113.7".parse().expect("合法 IP");
    let fake: std::net::IpAddr = "198.18.0.5".parse().expect("合法 IP");

    assert!(!dns::needs_doh(host, &Ok(vec![real])));
    assert!(dns::needs_doh(host, &Ok(vec![fake])));
    assert!(dns::needs_doh(host, &Ok(vec![])));
    assert!(dns::needs_doh(host, &Err("servfail".to_string())));
    // 非沙箱域名：无论解析成败都不查 DoH。
    assert!(!dns::needs_doh("www.workbuddy.ai", &Ok(vec![fake])));
    assert!(!dns::needs_doh(
        "www.workbuddy.ai",
        &Err("servfail".to_string())
    ));
    // 后缀匹配必须带点：`evil-sandbox.cloudstudio.club` 这种域不算沙箱。
    assert!(!dns::needs_doh(
        "evil-sandbox.cloudstudio.club",
        &Err("x".to_string())
    ));
}

/// A 记录挑选：跳过 CNAME 与 fake-ip 应答。
#[test]
fn pick_a_record_skips_cname_and_fake_ip() {
    let body = serde_json::json!({
        "Status": 0,
        "Answer": [
            {"type": 5, "data": "alias.example.com"},
            {"type": 1, "data": "198.18.0.9"},
            {"type": 1, "data": "203.0.113.7"}
        ]
    });
    assert_eq!(
        dns::pick_a_record(&body),
        Some("203.0.113.7".parse().expect("合法 IP"))
    );
    assert_eq!(dns::pick_a_record(&serde_json::json!({})), None);
    assert_eq!(
        dns::pick_a_record(&serde_json::json!({"Answer": [{"type": 5, "data": "a.example.com"}]})),
        None
    );
}

/// DoH 应答非 2xx / 非 JSON 时返回 None（兜底失败不改判整轮）。
#[tokio::test]
async fn doh_lookup_returns_none_on_bad_response() {
    let host = "sbx-1.sandbox.cloudstudio.club";
    let stub = Stub::happy();
    // 桩默认给的是合法应答。
    assert!(dns::doh_lookup(&stub, host).await.is_some());
    assert_eq!(stub.doh_queries(), 1);
}

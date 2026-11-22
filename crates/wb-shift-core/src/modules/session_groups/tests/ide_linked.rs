// CodeBuddy IDE「新增关联会话」的客户端分派与失败文案。
//
// IDE 的数据根来自真实安装路径（`%APPDATA%/CodeBuddyExtension/...`），单测无法注入，
// 因此这里钉住的是**分派本身**：只要哪一天有人把 IDE 的 arm 拿掉、退回「暂不支持」，
// 下面每一条都会先失败——这正是 #195 的回归点。
//
// 与 `tests/editor.rs` 的 `copy_linked_sessions_rejects_invalid_requests_before_touching_editor`
// 互补：那边覆盖 WorkBuddy / VS Code 的旧路径，这里只覆盖 IDE 这一条新路。

#[test]
fn list_account_sessions_dispatches_to_codebuddy_ide_instead_of_refusing() {
    // 缺 uid 的账号：四个客户端都应停在「账号缺少 uid」。对 IDE 而言，能走到这一步就说明
    // 它不再在第一句就返回「当前客户端暂不支持列出账号会话」（改造前的行为）。
    let account = json!({ "id": "acc-without-uid" });
    for client in [
        SessionClient::Workbuddy,
        SessionClient::CodebuddyIde,
        SessionClient::VscodeExt,
        SessionClient::Jetbrains,
    ] {
        let error = list_account_sessions(client, &account).unwrap_err();
        assert_eq!(error, "账号缺少 uid", "client={}", client.as_str());
    }
}

#[test]
fn missing_data_root_reason_names_the_client_the_user_must_open() {
    // 数据根解析不到时，用户必须知道「去打开 / 登录哪个客户端」，否则这句话无解。
    let ide = missing_data_root_reason(SessionClient::CodebuddyIde);
    assert!(ide.contains("CodeBuddy IDE"), "{ide}");
    assert!(ide.contains("登录"), "{ide}");
    let vscode = missing_data_root_reason(SessionClient::VscodeExt);
    assert!(vscode.contains("VS Code"), "{vscode}");
    let jetbrains = missing_data_root_reason(SessionClient::Jetbrains);
    assert!(jetbrains.contains("JetBrains"), "{jetbrains}");
    // 三个客户端的文案必须互不相同：串用会指错客户端。
    assert_ne!(ide, vscode);
    assert_ne!(ide, jetbrains);
}

#[test]
fn codebuddy_ide_running_hint_asks_for_a_full_exit() {
    // IDE 不参与「关闭并重开」：只能要求用户先完全退出，且必须指名是哪个客户端。
    assert!(CODEBUDDY_IDE_ADD_LINKED_RUNNING_HINT.contains("CodeBuddy IDE"));
    assert!(CODEBUDDY_IDE_ADD_LINKED_RUNNING_HINT.contains("完全退出"));
    assert!(CODEBUDDY_IDE_ADD_LINKED_RUNNING_HINT.contains("新增关联会话"));
}

#[test]
fn ide_copy_rejects_target_account_of_the_other_flavor() {
    // 两个 IDE 共用一棵会话树，但关联组按档位隔离：档位不匹配必须拒绝，而不是默默登记到
    // 另一个档位的组里（那会让用户在刚打开的列表里找不到新建的组）。
    let intl_target = json!({ "uid": "uid-target", "variant": "ai" });
    let error = validate_target_compatibility(
        SessionClient::CodebuddyIde,
        Some(WbVariant::Cn),
        &intl_target,
    )
    .unwrap_err();
    assert!(error.contains("档位不兼容"), "{error}");

    // 档位一致时放行。
    let cn_target = json!({ "uid": "uid-target", "variant": "cn" });
    assert!(validate_target_compatibility(
        SessionClient::CodebuddyIde,
        Some(WbVariant::Cn),
        &cn_target
    )
    .is_ok());

    // 插件侧同一目标不拒绝：它的存储与地区无关（口径差异要一直保留）。
    assert!(validate_target_compatibility(SessionClient::VscodeExt, None, &intl_target).is_ok());
}

#[test]
fn copy_linked_sessions_only_refuses_workbuddy() {
    // 「暂不支持新增关联会话」这句话只能出现在 WorkBuddy 上：其余三个客户端都必须进入
    // 参数校验（空勾选 / 缺档位），而不是在入口处被一刀切掉。
    for client in [
        SessionClient::CodebuddyIde,
        SessionClient::VscodeExt,
        SessionClient::Jetbrains,
    ] {
        let error = copy_linked_sessions(client, None, "source", "target", &[], false).unwrap_err();
        assert!(
            !error.contains("暂不支持新增关联会话"),
            "client={}: {error}",
            client.as_str()
        );
    }
    // 空勾选在三个客户端上都报同一句话（插件侧不传档位、IDE 侧先被 validate_scope 拦下）。
    assert!(copy_linked_sessions(
        SessionClient::VscodeExt,
        None,
        "source",
        "target",
        &[],
        false
    )
    .unwrap_err()
    .contains("缺少要复制的会话"));
    assert!(copy_linked_sessions(
        SessionClient::CodebuddyIde,
        Some(WbVariant::Cn),
        "source",
        "target",
        &[],
        false
    )
    .unwrap_err()
    .contains("缺少要复制的会话"));
}

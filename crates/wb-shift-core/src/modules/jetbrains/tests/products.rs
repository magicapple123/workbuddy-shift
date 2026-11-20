// R5-19：JetBrains 全家桶 + Android Studio 的识别面（产品表 / 双配置根 / 目录名回退）。
//
// 这些用例全部自己造目录、自己传根，不读真实 `%APPDATA%`：CI 上没有 IDE，
// 用户机上又可能有真实配置目录，两边都会让断言不稳（AGENTS.md 第 8 节）。

/// 造一个隔离的临时根：先清空再创建，避免上一次失败的残留影响本次断言。
fn p_temp_root(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "workbuddy-shift-jb-r519-{}-{tag}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

/// 在 `path` 下造一个装了 CodeBuddy 插件的配置目录。
fn p_make_config_dir(path: &Path) {
    std::fs::create_dir_all(path.join("plugins/Tencent-Cloud.coding-copilot")).unwrap();
}

/// 产品表必须覆盖 16 个产品，且 `PRODUCT_PREFIXES` 与 `PRODUCTS` 一一对应。
///
/// 这条用例是「只补了一部分产品」的拦截网：PR #208 的产品清单在移植讨论里改过几次，
/// 表与派生常量一旦不同步，就会出现「目录名判定接受了、但进程名匹配不到」的半状态。
#[test]
fn product_table_covers_sixteen_products() {
    assert_eq!(PRODUCTS.len(), 16, "产品表必须正好 16 行");
    assert_eq!(PRODUCT_PREFIXES.len(), PRODUCTS.len());
    for (index, product) in PRODUCTS.iter().enumerate() {
        assert_eq!(
            PRODUCT_PREFIXES[index], product.prefix,
            "派生前缀必须与产品表同序：{index}"
        );
        assert!(
            !product.windows_exes.is_empty(),
            "{} 缺少 Windows 映像名",
            product.prefix
        );
        assert!(!product.macos_app.is_empty(), "{} 缺少 .app 名", product.prefix);
        assert!(!product.linux_bin.is_empty(), "{} 缺少 Linux 可执行名", product.prefix);
    }
    let expected = [
        "IntelliJIdea",
        "IdeaIC",
        "PyCharm",
        "PyCharmCE",
        "WebStorm",
        "PhpStorm",
        "GoLand",
        "CLion",
        "RubyMine",
        "DataGrip",
        "Rider",
        "Aqua",
        "DataSpell",
        "AppCode",
        "MPS",
        "AndroidStudio",
    ];
    for prefix in expected {
        assert!(
            PRODUCT_PREFIXES.contains(&prefix),
            "产品表缺少 {prefix}（R5-19 决策要求 16 个）"
        );
    }
}

/// 16 个产品各自的 `<产品><版本>` 配置目录名都必须被接受。
#[test]
fn every_product_accepts_versioned_config_dir_name() {
    for prefix in PRODUCT_PREFIXES {
        for version in ["2024.1", "2025.2.1", "2026.3-eap"] {
            let name = format!("{prefix}{version}");
            assert!(is_supported_config_dir_name(&name), "{name} 应被接受");
        }
        // 只有前缀、没有版本号的目录名仍然拒绝（那是安装残留，不是配置目录）。
        assert!(
            !is_supported_config_dir_name(prefix),
            "{prefix} 没有版本号，应被拒绝"
        );
    }
}

/// Android Studio：`AndroidStudio<版本>` 是唯一可接受的形态，无关目录必须拒绝。
///
/// 验收口径（清单 R5-19）：「人工创建 `AndroidStudio2024.1` 目录 + 无 product-info」
/// 能被识别；`Chrome` / `Google` 这类同根下的无关目录被拒绝。
#[test]
fn android_studio_dir_name_fallback_is_accepted_and_noise_rejected() {
    for name in ["AndroidStudio2024.1", "AndroidStudio2023.3.1", "AndroidStudio2026.1-eap"] {
        assert!(is_supported_config_dir_name(name), "{name} 应被接受");
    }
    for name in [
        "AndroidStudio",
        "AndroidStudioX",
        // 纯主版本号「2024」是合法版本后缀（与 `PyCharm2024` 同口径），不算噪声。
        "AndroidStudio-2024.1",
        "AndroidStudio2024.1-backup",
        "Chrome",
        "Google",
        "Android",
    ] {
        assert!(!is_supported_config_dir_name(name), "{name} 应被拒绝");
    }
}

/// exe / 可执行名 → 产品前缀：三平台命名都要覆盖，未知映像不得误判。
#[test]
fn product_prefix_from_exe_covers_all_products_and_platforms() {
    for (name, expected) in [
        // Windows 64 位映像
        ("idea64.exe", "IntelliJIdea"),
        ("pycharm64.exe", "PyCharm"),
        ("webstorm64.exe", "WebStorm"),
        ("phpstorm64.exe", "PhpStorm"),
        ("goland64.exe", "GoLand"),
        ("clion64.exe", "CLion"),
        ("rubymine64.exe", "RubyMine"),
        ("datagrip64.exe", "DataGrip"),
        ("rider64.exe", "Rider"),
        ("aqua64.exe", "Aqua"),
        ("dataspell64.exe", "DataSpell"),
        ("appcode64.exe", "AppCode"),
        ("mps64.exe", "MPS"),
        ("studio64.exe", "AndroidStudio"),
        // 历史 32 位映像
        ("idea.exe", "IntelliJIdea"),
        ("studio.exe", "AndroidStudio"),
        // Linux 可执行名（`<root>/bin/<x>`，无 .exe）
        ("idea", "IntelliJIdea"),
        ("goland", "GoLand"),
        ("studio", "AndroidStudio"),
        ("appcode", "AppCode"),
        // 大小写不敏感（macOS / Linux 上不一定全小写）
        ("GoLand64.EXE", "GoLand"),
    ] {
        assert_eq!(
            product_prefix_from_exe_name(name),
            Some(expected),
            "{name} 应映射到 {expected}"
        );
    }
    for name in ["chrome.exe", "code.exe", "explorer.exe", "java.exe", ""] {
        assert_eq!(product_prefix_from_exe_name(name), None, "{name} 不应被识别");
    }
}

/// Windows 映像名清单由产品表派生：16 个产品都在，且没有重复项。
#[cfg(target_os = "windows")]
#[test]
fn windows_image_names_are_derived_from_product_table() {
    let images = windows_image_names();
    for expected in [
        "idea64.exe",
        "pycharm64.exe",
        "webstorm64.exe",
        "phpstorm64.exe",
        "goland64.exe",
        "clion64.exe",
        "rubymine64.exe",
        "datagrip64.exe",
        "rider64.exe",
        "aqua64.exe",
        "dataspell64.exe",
        "appcode64.exe",
        "mps64.exe",
        "studio64.exe",
        // 32 位历史映像
        "idea.exe",
        "pycharm.exe",
        "studio.exe",
        "appcode.exe",
    ] {
        assert!(images.contains(&expected), "映像清单缺少 {expected}");
    }
    let mut sorted = images.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), images.len(), "映像清单不能有重复项");
    // 16 个产品 × 2 种位宽 = 32 条；IDEA / IDEA CE 与 PyCharm / PyCharm CE 共用同一套
    // 映像名（社区版只能靠安装目录名或 product-info 区分），去重后少 4 条。
    assert_eq!(images.len(), 28, "应为 16 个产品 × 2 种位宽 - 4 条品牌共享");
}

/// 配置根必须是「JetBrains + Google」两处，且同属一个平台基础目录。
#[test]
fn config_roots_cover_jetbrains_and_google() {
    let roots = jetbrains_config_roots();
    assert_eq!(roots.len(), 2, "必须返回两个配置根：{roots:?}");
    let names: Vec<String> = roots
        .iter()
        .map(|root| {
            root.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
        .collect();
    assert_eq!(names, vec!["JetBrains".to_string(), "Google".to_string()]);
    let base = roots[0].parent().expect("配置根必须有平台基础目录");
    assert_eq!(
        roots[1].parent().map(Path::to_path_buf),
        Some(base.to_path_buf()),
        "两个根必须并列在同一个平台基础目录下"
    );
}

/// JetBrains 根不存在、只有 Google 根存在时，Android Studio 仍然要出现在切换列表里。
///
/// 这是双根改造的核心验收：只扫第一个根会把只装了 AS 的用户整体判成「未安装」
/// （`status().installed` 就是用 `list_targets()` 是否为空算出来的）。
#[test]
fn list_targets_reads_android_studio_from_google_root_only() {
    let base = p_temp_root("google-only");
    let jetbrains_root = base.join("JetBrains"); // 故意不创建
    let google_root = base.join("Google");
    p_make_config_dir(&google_root.join("AndroidStudio2024.1"));
    // 同根下的无关目录：既不能进列表，也不能让 AS 掉出列表。
    std::fs::create_dir_all(google_root.join("Chrome")).unwrap();
    std::fs::create_dir_all(google_root.join("AndroidStudioX")).unwrap();

    assert!(!jetbrains_root.exists());

    let targets = list_targets_from(&[jetbrains_root.clone(), google_root.clone()]);
    let names: Vec<&str> = targets.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, vec!["AndroidStudio2024.1"], "只应列出 AS 目录：{names:?}");
    assert!(targets[0].plugin_installed, "插件目录已造好，应判为已安装");
    assert_eq!(
        targets[0].config_dir,
        google_root.join("AndroidStudio2024.1"),
        "配置目录必须指向 Google 根"
    );

    // 诊断扫描同口径：AS 被接受，Chrome / AndroidStudioX 被拒但仍然可见（供排查）。
    let scans = scan_config_roots_from(&[jetbrains_root, google_root]);
    let scan_names: Vec<&str> = scans.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        scan_names,
        vec!["AndroidStudio2024.1", "AndroidStudioX", "Chrome"],
        "诊断清单必须列出全部子目录：{scan_names:?}"
    );
    for scan in &scans {
        let expected = scan.name == "AndroidStudio2024.1";
        assert_eq!(scan.accepted, expected, "{} 的接受判定错误", scan.name);
    }

    let _ = std::fs::remove_dir_all(&base);
}

/// 两个根下同名目录（迁移残留）只认先命中的那个，避免同名目标写两次。
#[test]
fn list_targets_dedupes_same_config_dir_across_roots() {
    let base = p_temp_root("dedupe");
    let jetbrains_root = base.join("JetBrains");
    let google_root = base.join("Google");
    p_make_config_dir(&jetbrains_root.join("PyCharm2026.2"));
    p_make_config_dir(&google_root.join("PyCharm2026.2"));

    let targets = list_targets_from(&[jetbrains_root.clone(), google_root]);
    assert_eq!(targets.len(), 1, "同名目录只应保留一个：{targets:?}");
    assert!(
        targets[0].config_dir.starts_with(&jetbrains_root),
        "JetBrains 根优先：{:?}",
        targets[0].config_dir
    );

    let _ = std::fs::remove_dir_all(&base);
}

/// 兜底映射只对 IDEA / PyCharm 做 community → CE，其余产品保持原前缀。
#[test]
fn fallback_keeps_original_prefix_for_non_idea_products() {
    let base = p_temp_root("fallback-prefix");

    // GoLand 安装目录名里带 community 字样（现实中不存在，但规则必须不误伤）。
    let goland_root = base.join("GoLand 2024.1");
    std::fs::create_dir_all(goland_root.join("bin")).unwrap();
    let goland_exe = goland_root.join("bin/goland64.exe");
    std::fs::write(&goland_exe, "").unwrap();
    assert_eq!(
        resolve_config_dir(&goland_root, Some(&goland_exe))
            .name
            .as_deref(),
        Some("GoLand2024.1")
    );

    // WebStorm 同样不得被套上 CE 后缀。
    let webstorm_root = base.join("WebStorm 2025.2");
    std::fs::create_dir_all(webstorm_root.join("bin")).unwrap();
    let webstorm_exe = webstorm_root.join("bin/webstorm64.exe");
    std::fs::write(&webstorm_exe, "").unwrap();
    assert_eq!(
        resolve_config_dir(&webstorm_root, Some(&webstorm_exe))
            .name
            .as_deref(),
        Some("WebStorm2025.2")
    );

    // IDEA / PyCharm 的社区版映射保持不变。
    let idea_ce = base.join("IntelliJ IDEA Community Edition 2026.3");
    std::fs::create_dir_all(idea_ce.join("bin")).unwrap();
    let idea_exe = idea_ce.join("bin/idea64.exe");
    std::fs::write(&idea_exe, "").unwrap();
    assert_eq!(
        resolve_config_dir(&idea_ce, Some(&idea_exe)).name.as_deref(),
        Some("IdeaIC2026.3")
    );

    let _ = std::fs::remove_dir_all(&base);
}

/// Android Studio：优先 product-info；缺失时**不猜**（保留 unmapped 让切换被阻断）。
///
/// AS 的安装目录名里没有版本号（`C:\Program Files\Android\Android Studio`），
/// 拿不到 `dataDirectoryName` 就无法知道该写 `AndroidStudio2024.1` 还是 `2023.3`：
/// 猜错会写到不存在的目录，静默变成「切了没变」。因此这里必须落到 `Unmapped`。
#[test]
fn android_studio_prefers_product_info_and_never_guesses() {
    let base = p_temp_root("as-mapping");

    // 1) 有 product-info：精确映射。
    let info_root = base.join("Android Studio");
    std::fs::create_dir_all(info_root.join("bin")).unwrap();
    std::fs::write(
        info_root.join("product-info.json"),
        r#"{"name":"Android Studio","version":"2024.1","dataDirectoryName":"AndroidStudio2024.1"}"#,
    )
    .unwrap();
    let exe = info_root.join("bin/studio64.exe");
    std::fs::write(&exe, "").unwrap();
    let mapping = resolve_config_dir(&info_root, Some(&exe));
    assert_eq!(mapping.name.as_deref(), Some("AndroidStudio2024.1"));
    assert_eq!(mapping.source, Some(MappingSource::ProductInfo));

    // 2) 没有 product-info、安装目录名也没有版本号：不猜，标 unmapped 并留原因。
    let bare_root = base.join("Android Studio Beta");
    std::fs::create_dir_all(bare_root.join("bin")).unwrap();
    let bare_exe = bare_root.join("bin/studio64.exe");
    std::fs::write(&bare_exe, "").unwrap();
    let unmapped = resolve_config_dir(&bare_root, Some(&bare_exe));
    assert_eq!(unmapped.name, None);
    assert_eq!(unmapped.source, Some(MappingSource::Unmapped));
    assert!(
        unmapped.note.is_some(),
        "映射失败必须留下原因供诊断展示"
    );

    // 3) 目录名本身带版本号时（移植 PR 的兜底口径）仍能推断出产品前缀。
    assert_eq!(
        product_prefix_from_exe_name("studio64.exe"),
        Some("AndroidStudio"),
        "AS 的映像名必须映射到 AndroidStudio 前缀"
    );

    let _ = std::fs::remove_dir_all(&base);
}

/// 诊断输出里必须同时有 `configRoots` 与 `rootStates`（两个根各一条状态）。
///
/// `status` 与 `diagnose` 是两个独立字段：漏一处前端就是 `undefined`
/// （清单 R5-19「已知坑」第 2 条）。
#[test]
fn diagnostics_json_exposes_both_roots_with_state() {
    let value = diagnostics_json(&[], &[]);
    let roots = value["configRoots"]
        .as_array()
        .expect("configRoots 必须是数组");
    assert_eq!(roots.len(), 2, "两个配置根都要输出：{value}");
    let states = value["rootStates"]
        .as_array()
        .expect("rootStates 必须是数组");
    assert_eq!(states.len(), 2);
    for (state, root) in states.iter().zip(roots.iter()) {
        assert_eq!(state["path"], *root, "rootStates 与 configRoots 必须一一对应");
        assert!(
            state["exists"].is_boolean(),
            "存在性必须是布尔值：{state}"
        );
    }
    // 字段集合必须**恰好**是这 7 个：旧单根字段一旦回流（哪怕以别名形态）这里就会红，
    // 前端拿到的就是 `undefined`（「已知坑」第 2 条）。用「键集合全等」而不是逐个否定，
    // 既覆盖了删除，也拦住了新加的未登记字段。
    let mut keys: Vec<&str> = value
        .as_object()
        .expect("diagnostics 必须是对象")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec![
            "acceptedDirs",
            "configRoots",
            "pluginInstalledDirs",
            "rootStates",
            "running",
            "scannedDirs",
            "unmappedRunningCount",
        ],
        "diagnostics 的字段集合被改动过：{value}"
    );
}

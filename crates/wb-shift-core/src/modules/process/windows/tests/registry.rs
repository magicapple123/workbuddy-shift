    /// Uninstall 分支：DisplayName 命中档位匹配、DisplayIcon 指向自定义目录 exe
    /// （带 `,0` 图标索引形态）。
    #[cfg(target_os = "windows")]
    #[test]
    fn windows_registry_probe_resolves_custom_dir_uninstall_entry() {
        let _guard = REGISTRY_PROBE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut fx = RegistryProbeFixture::new("uninstall");
        let exe = fx.stub_exe(&fx.dir, "WorkBuddy.exe");
        let key = format!(
            r"{TEST_UNINSTALL_ROOT}\workbuddy-shift-probe-uninstall-{}",
            std::process::id()
        );
        fx.write_uninstall(&key, "WorkBuddy", &format!("\"{}\",0", exe.display()));

        let cands = probe_candidates_for_test(WbVariant::Cn);
        assert!(
            cands.iter().any(|p| p == &exe),
            "Uninstall DisplayIcon 未解析出自定义目录 exe（期望 {}）: 候选={cands:?}",
            exe.display()
        );

        fx.cleanup_and_verify(&[WbVariant::Cn]);
    }

    /// App Paths 分支：默认值指向自定义目录 exe。
    #[cfg(target_os = "windows")]
    #[test]
    fn windows_registry_probe_resolves_custom_dir_app_paths_entry() {
        let _guard = REGISTRY_PROBE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut fx = RegistryProbeFixture::new("app-paths");
        let exe = fx.stub_exe(&fx.dir, "WorkBuddy.exe");
        fx.write_app_paths("WorkBuddy.exe", &exe.to_string_lossy());

        let cands = probe_candidates_for_test(WbVariant::Cn);
        assert!(
            cands.iter().any(|p| p == &exe),
            "App Paths 默认值未解析出自定义目录 exe（期望 {}）: 候选={cands:?}",
            exe.display()
        );

        fx.cleanup_and_verify(&[WbVariant::Cn]);
    }

    /// 档位隔离：CN/AI 合成项并存时互不串档。
    #[cfg(target_os = "windows")]
    #[test]
    fn windows_registry_probe_isolates_variants() {
        let _guard = REGISTRY_PROBE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut fx = RegistryProbeFixture::new("variants");
        let cn_dir = fx.subdir("cn");
        let ai_dir = fx.subdir("ai");
        let cn_exe = fx.stub_exe(&cn_dir, "WorkBuddy.exe");
        let ai_exe = fx.stub_exe(&ai_dir, "WorkBuddyAI.exe");

        let pid = std::process::id();
        fx.write_uninstall(
            &format!(r"{TEST_UNINSTALL_ROOT}\workbuddy-shift-probe-cn-{pid}"),
            "WorkBuddy",
            &format!("\"{}\",0", cn_exe.display()),
        );
        fx.write_uninstall(
            &format!(r"{TEST_UNINSTALL_ROOT}\workbuddy-shift-probe-ai-{pid}"),
            "WorkBuddyAI",
            &format!("\"{}\",0", ai_exe.display()),
        );
        fx.write_app_paths("WorkBuddyAI.exe", &ai_exe.to_string_lossy());

        let cn = probe_candidates_for_test(WbVariant::Cn);
        assert!(
            cn.iter().any(|p| p == &cn_exe),
            "CN 未解析出自定义目录 exe: 候选={cn:?}"
        );
        assert!(
            !cn.iter().any(|p| p.starts_with(&ai_dir)),
            "CN 档位串到国际版合成目录: 候选={cn:?}"
        );

        let ai = probe_candidates_for_test(WbVariant::Ai);
        assert!(
            ai.iter().any(|p| p == &ai_exe),
            "AI 未解析出自定义目录 exe: 候选={ai:?}"
        );
        assert!(
            !ai.iter().any(|p| p.starts_with(&cn_dir)),
            "AI 档位串到国内版合成目录: 候选={ai:?}"
        );

        fx.cleanup_and_verify(&WbVariant::ALL);
    }

    /// 自排除：指向 workbuddy-shift.exe 的条目不得被命中。
    #[cfg(target_os = "windows")]
    #[test]
    fn windows_registry_probe_excludes_self_entries() {
        let _guard = REGISTRY_PROBE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut fx = RegistryProbeFixture::new("self");
        let self_exe = fx.stub_exe(&fx.dir, "workbuddy-shift.exe");
        let real_exe = fx.stub_exe(&fx.dir, "WorkBuddy.exe");

        let pid = std::process::id();
        // DisplayName 故意写成档位名：确保条目被探测脚本输出，由解析层的自排除拦截。
        fx.write_uninstall(
            &format!(r"{TEST_UNINSTALL_ROOT}\workbuddy-shift-probe-self-{pid}"),
            "WorkBuddy",
            &self_exe.to_string_lossy(),
        );
        fx.write_uninstall(
            &format!(r"{TEST_UNINSTALL_ROOT}\workbuddy-shift-probe-real-{pid}"),
            "WorkBuddy",
            &real_exe.to_string_lossy(),
        );

        let cn = probe_candidates_for_test(WbVariant::Cn);
        assert!(
            cn.iter().any(|p| p == &real_exe),
            "对照组条目未被命中，无法证明探测链路工作: 候选={cn:?}"
        );
        assert!(
            !cn.iter().any(|p| p == &self_exe),
            "自排除失效，命中 workbuddy-shift.exe: 候选={cn:?}"
        );

        fx.cleanup_and_verify(&[WbVariant::Cn]);
    }

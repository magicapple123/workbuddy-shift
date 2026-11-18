    #[test]
    fn registry_lines_parse_display_icon_and_skip_self() {
        let stdout = r#"
"D:\Users\Zhou\AppData\Local\Programs\WorkBuddy\WorkBuddy.exe,0"
C:\Users\Zhou\AppData\Local\Programs\workbuddy-shift\workbuddy-shift.exe
D:\Program Files\WorkBuddy\Uninstall.exe
D:\Users\Zhou\AppData\Local\Programs\WorkBuddy\WorkBuddy.exe
"#;
        let paths = parse_windows_registry_path_lines(stdout, WbVariant::Cn);
        let s: Vec<String> = paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            s,
            vec![r"D:\Users\Zhou\AppData\Local\Programs\WorkBuddy\WorkBuddy.exe".to_string(),]
        );
        assert!(parse_windows_registry_path_lines(stdout, WbVariant::Ai).is_empty());
    }

    // ------------------------------------------------------------------
    // Windows 注册表探测：合成 HKCU 项（windows-latest 真机取证）
    //
    // 本机是 macOS，注册表 + PowerShell 这条链路只能在 Windows 上运行验证。
    // 只调用候选层 `windows_registry_exe_candidates`，不碰
    // `windows_workbuddy_exe_path`（后者会写 ~/.workbuddy-shift 缓存、污染开发机）。
    // Uninstall 子键带 pid，写入前先删同名键。App Paths 必须用真实 exe 名
    // （探测脚本按这个名字查找），不是 pid 键；覆盖前先读出原默认值，清理时写回。
    // 夹具 Drop 无条件清理（断言 panic 也会执行）。
    // ------------------------------------------------------------------

    #[cfg(target_os = "windows")]
    const TEST_UNINSTALL_ROOT: &str = r"HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";
    #[cfg(target_os = "windows")]
    const TEST_APP_PATHS_ROOT: &str = r"HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths";

    /// 本组用例每个都要多次启动 PowerShell/reg.exe；CI（windows-latest）上并发
    /// 启动会互相挤占，曾出现 8s/15s 超时的假失败（同代码重跑即过）。与
    /// cli_auth_sync_e2e 的 ENV_LOCK 同一思路：组内用例串行执行。
    #[cfg(target_os = "windows")]
    static REGISTRY_PROBE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 执行 reg.exe（best-effort，清理路径用：失败不 panic）。
    #[cfg(target_os = "windows")]
    fn reg_try(args: &[&str]) {
        let _ = run_cmd_timeout("reg.exe", args, 15);
    }

    /// 执行 reg.exe 并断言成功（写入路径用）。
    #[cfg(target_os = "windows")]
    fn reg_run(args: &[&str]) {
        let out = run_cmd_timeout("reg.exe", args, 15).expect("reg.exe 未能启动或超时");
        assert!(
            out.status.success(),
            "reg {args:?} 失败: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }

    #[cfg(target_os = "windows")]
    fn app_paths_key(exe_name: &str) -> String {
        format!(r"{TEST_APP_PATHS_ROOT}\{exe_name}")
    }

    /// 读取 App Paths 默认值，供清理时恢复。
    ///
    /// 键不存在或默认值为空 → `None`（后续可以新建）。
    /// powershell 没启动或超时则 panic：不能把读取失败当成「没有原值」再删键。
    #[cfg(target_os = "windows")]
    fn read_app_paths_default(exe_name: &str) -> Option<String> {
        let script = format!(
            "$ErrorActionPreference = 'SilentlyContinue'; \
             $p = Get-ItemProperty -LiteralPath 'HKCU:\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths\\{exe_name}'; \
             if ($null -eq $p) {{ 'MISSING' }} \
             elseif ($null -eq $p.'(default)' -or [string]$p.'(default)' -eq '') {{ 'NODEFAULT' }} \
             else {{ 'VALUE:' + [string]$p.'(default)' }}"
        );
        let out = ps_output(&script, 60)
            .expect("读取 App Paths 原默认值失败（powershell 60s 未返回），中止以避免误删现有键");
        let value = out.trim();
        if value == "MISSING" || value == "NODEFAULT" {
            return None;
        }
        let Some(raw) = value.strip_prefix("VALUE:") else {
            panic!("读取 App Paths 原默认值得到无法识别的输出，中止以避免误删现有键: {value:?}");
        };
        if raw.is_empty() {
            None
        } else {
            Some(raw.to_string())
        }
    }

    /// 测试专用注册表探测：与生产同链路，但超时放宽到 60s 并在超时时显式失败。
    ///
    /// 生产侧 8s 是给真实用户的等待兜底，不因 CI 放宽；测试侧把「超时」与
    /// 「探测为空」区分开，避免把环境抖动误报成功能回归（曾出现候选=[] 的假失败）。
    #[cfg(target_os = "windows")]
    fn probe_candidates_for_test(variant: WbVariant) -> Vec<PathBuf> {
        let Some(stdout) = ps_output(&windows_registry_probe_script(variant), 60) else {
            panic!("注册表探测 PowerShell 60s 未返回，无法判定功能是否正常");
        };
        parse_windows_registry_path_lines(&stdout, variant)
    }

    /// 合成注册表项 + 临时目录的 RAII 夹具：Drop 时无条件清理。
    #[cfg(target_os = "windows")]
    struct RegistryProbeFixture {
        /// 合成目录（temp_dir 下的自定义子目录，不在任何扫描白名单内）。
        dir: PathBuf,
        /// 清理时整键删除的键（Uninstall 合成项）。
        keys: Vec<String>,
        /// 清理时恢复原默认值的 App Paths 键：(exe 名, 原默认值)。
        app_paths: Vec<(String, Option<String>)>,
        cleaned: bool,
    }

    #[cfg(target_os = "windows")]
    impl RegistryProbeFixture {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "workbuddy-shift-registry-probe-{tag}-{}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).expect("创建合成目录失败");
            Self {
                dir,
                keys: Vec::new(),
                app_paths: Vec::new(),
                cleaned: false,
            }
        }

        fn subdir(&self, name: &str) -> PathBuf {
            let d = self.dir.join(name);
            std::fs::create_dir_all(&d).expect("创建合成子目录失败");
            d
        }

        /// 在合成目录下放一个空 exe。
        fn stub_exe(&self, dir: &Path, file_name: &str) -> PathBuf {
            let p = dir.join(file_name);
            std::fs::write(&p, b"").expect("创建合成 exe 失败");
            p
        }

        /// 写 Uninstall 合成项（先登记后写入：写入中途失败也会被清理）。
        fn write_uninstall(&mut self, key: &str, display_name: &str, display_icon: &str) {
            self.keys.push(key.to_string());
            reg_try(&["delete", key, "/f"]); // 重复运行时先清同名键
            reg_run(&[
                "add",
                key,
                "/v",
                "DisplayName",
                "/t",
                "REG_SZ",
                "/d",
                display_name,
                "/f",
            ]);
            reg_run(&[
                "add",
                key,
                "/v",
                "DisplayIcon",
                "/t",
                "REG_SZ",
                "/d",
                display_icon,
                "/f",
            ]);
        }

        /// 写 App Paths 合成项（默认值 = exe 路径）；原默认值登记待恢复。
        fn write_app_paths(&mut self, exe_name: &str, target: &str) {
            let prev = read_app_paths_default(exe_name);
            self.app_paths.push((exe_name.to_string(), prev));
            let key = app_paths_key(exe_name);
            reg_try(&["delete", key.as_str(), "/f"]); // 重复运行时先清同名键
            reg_run(&[
                "add",
                key.as_str(),
                "/ve",
                "/t",
                "REG_SZ",
                "/d",
                target,
                "/f",
            ]);
        }

        fn cleanup(&mut self) {
            if self.cleaned {
                return;
            }
            self.cleaned = true;
            for key in &self.keys {
                reg_try(&["delete", key.as_str(), "/f"]);
            }
            // 开发机可能真装了 WorkBuddy：App Paths 键不能一删了之，恢复原默认值。
            // 已知限制：只恢复默认值，原键若还带 `Path` 等值会被一并丢弃（CI 无真实安装，
            // 不受影响；开发机影响面为 App Paths 的按名解析，主程序仍可由默认值启动）。
            for (exe_name, prev) in &self.app_paths {
                let key = app_paths_key(exe_name);
                reg_try(&["delete", key.as_str(), "/f"]);
                if let Some(value) = prev {
                    reg_try(&[
                        "add",
                        key.as_str(),
                        "/ve",
                        "/t",
                        "REG_SZ",
                        "/d",
                        value.as_str(),
                        "/f",
                    ]);
                }
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }

        /// 显式清理后重新探测：合成路径必须彻底消失（清理有效性的自检）。
        fn cleanup_and_verify(&mut self, variants: &[WbVariant]) {
            self.cleanup();
            for variant in variants {
                let after = probe_candidates_for_test(*variant);
                assert!(
                    !after.iter().any(|p| p.starts_with(&self.dir)),
                    "清理后仍能探测到合成路径（{variant:?}）: {after:?}"
                );
            }
        }
    }

    #[cfg(target_os = "windows")]
    impl Drop for RegistryProbeFixture {
        fn drop(&mut self) {
            self.cleanup();
        }
    }

    #[test]
    fn codebuddy_cli_process_args_match_package_and_skip_ide() {
        assert!(is_codebuddy_cli_process_args(
            "node /Users/me/.nvm/versions/node/v22.20.0/lib/node_modules/@tencent-ai/codebuddy-code/dist-server/codebuddy.js"
        ));
        assert!(is_codebuddy_cli_process_args(
            "node C:\\Users\\me\\AppData\\Roaming\\npm\\node_modules\\@tencent-ai\\codebuddy-code\\bin\\codebuddy"
        ));
        assert!(is_codebuddy_cli_process_args(
            "/Users/me/.nvm/versions/node/v22.20.0/lib/node_modules/@tencent-ai/codebuddy-code/bin/cbc-prewarm"
        ));
        assert!(is_codebuddy_cli_process_args(
            "node /Users/me/.nvm/versions/node/v22.20.0/bin/codebuddy --acp"
        ));
        assert!(!is_codebuddy_cli_process_args(
            "/Applications/CodeBuddy.app/Contents/MacOS/CodeBuddy"
        ));
        assert!(!is_codebuddy_cli_process_args(
            "/Applications/CodeBuddy CN.app/Contents/MacOS/CodeBuddy CN"
        ));
        assert!(!is_codebuddy_cli_process_args(
            r"C:\Users\me\AppData\Local\Programs\CodeBuddy\CodeBuddy.exe"
        ));
        assert!(!is_codebuddy_cli_process_args(
            "node /Users/me/Documents/github-project/workbuddy-shift/src-tauri"
        ));
    }

    #[test]
    fn apply_cli_region_env_writes_internal_for_cn_and_public_for_ai() {
        let mut settings = json!({
            "env": {
                "HTTPS_PROXY": "http://127.0.0.1:7890",
                CODEBUDDY_INTERNET_ENVIRONMENT: "internal",
                CODEBUDDY_BASE_URL: AI_CLI_ENDPOINT
            }
        });
        apply_cli_region_env(&mut settings, WbVariant::Ai).unwrap();
        assert_eq!(
            settings["env"][CODEBUDDY_INTERNET_ENVIRONMENT],
            AI_INTERNET_ENVIRONMENT
        );
        assert_eq!(settings["env"][CODEBUDDY_BASE_URL], AI_CLI_OPENAI_BASE_URL);
        assert_eq!(settings["env"]["HTTPS_PROXY"], "http://127.0.0.1:7890");
        apply_cli_region_env(&mut settings, WbVariant::Cn).unwrap();
        assert_eq!(
            settings["env"][CODEBUDDY_INTERNET_ENVIRONMENT],
            CN_INTERNET_ENVIRONMENT
        );
        assert!(settings["env"].get(CODEBUDDY_BASE_URL).is_none());
    }

    #[test]
    fn apply_cli_region_env_replaces_portal_root_base_url_with_openai_v2() {
        let mut settings = json!({
            "env": { CODEBUDDY_BASE_URL: "https://www.codebuddy.ai" }
        });
        apply_cli_region_env(&mut settings, WbVariant::Ai).unwrap();
        assert_eq!(
            settings["env"][CODEBUDDY_BASE_URL],
            "https://www.codebuddy.ai/v2"
        );
    }

    #[test]
    fn sync_cli_runtime_cache_rewrites_endpoint_and_drops_internal_env_for_ai() {
        let dir = helper_test_dir().join("local_storage");
        fs::create_dir_all(&dir).unwrap();
        let env_path = dir.join(cli_cache_filename(CLI_ENV_CACHE_KEY));
        let endpoint_path = dir.join(cli_cache_filename(CLI_ENDPOINT_CACHE_KEY));
        let product_path = dir.join(cli_cache_filename(CLI_PRODUCT_CACHE_KEY));
        fs::write(&env_path, "\"internal\"").unwrap();
        fs::write(&endpoint_path, "\"https://copilot.tencent.com\"").unwrap();
        fs::write(&product_path, "gzip-placeholder").unwrap();

        sync_cli_runtime_cache_at(&dir, WbVariant::Ai).unwrap();
        assert!(!product_path.exists());
        assert_eq!(
            fs::read_to_string(&env_path).unwrap(),
            serde_json::to_string(&json!(AI_INTERNET_ENVIRONMENT)).unwrap()
        );
        assert_eq!(
            fs::read_to_string(&endpoint_path).unwrap(),
            serde_json::to_string(&json!(AI_CLI_ENDPOINT)).unwrap()
        );

        sync_cli_runtime_cache_at(&dir, WbVariant::Cn).unwrap();
        assert_eq!(
            fs::read_to_string(&env_path).unwrap(),
            serde_json::to_string(&json!(CN_INTERNET_ENVIRONMENT)).unwrap()
        );
        assert_eq!(
            fs::read_to_string(&endpoint_path).unwrap(),
            serde_json::to_string(&json!(CN_CLI_ENDPOINT)).unwrap()
        );
        fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

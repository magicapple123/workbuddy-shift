    // 启动 / 存活探测用例（缺陷二），与 `tests/switch.rs` 一起被 `include!`，共用其
    // `instance_with_pids` 夹具；单开文件是为了让 `tests/switch.rs` 留在长文件阈值内，
    // 而且这里测的是 `status.rs` 的 `launch_vscode_with`，本来就不是切换语义。

    /// ① 探针只 true → Ok，且启动动作只被调用一次。
    ///
    /// 探针与启动动作都是注入的：真实实现会**真的启动用户的 VS Code**，测试绝不能走到那一步。
    #[test]
    fn relaunch_succeeds_when_probe_sees_liveness() {
        let inst = instance_with_pids(vec![4242]);
        let policy = LivenessPolicy {
            timeout: Duration::from_millis(50),
            interval: Duration::ZERO,
        };
        let mut spawns = 0usize;
        let result = launch_vscode_with(
            &inst,
            policy,
            |_| {
                spawns += 1;
                Ok(())
            },
            || true,
        );
        result.expect("探测到存活即应成功");
        assert_eq!(spawns, 1, "启动动作只该调用一次");
    }

    /// ② 探针只 false → Err，且文案**分开两件事**：写入已生效 / 编辑器启动后立即退出。
    ///
    /// 绝不能把已成功的写入说成失败——用户会因为「失败」而重复切号（再关一次编辑器、
    /// 再写一次库），而库其实早就写好了。
    #[test]
    fn relaunch_reports_immediate_exit_without_claiming_write_failed() {
        let inst = instance_with_pids(vec![4242]);
        // timeout = 0 ⇒ 探一次就按超时返回，用例不会真的等 5 秒。
        let policy = LivenessPolicy {
            timeout: Duration::ZERO,
            interval: Duration::ZERO,
        };
        let error = launch_vscode_with(&inst, policy, |_| Ok(()), || false)
            .expect_err("探测不到存活必须返回错误");
        assert!(error.contains("写入已生效"), "{error}");
        assert!(error.contains("编辑器启动后立即退出"), "{error}");
        assert!(error.contains("可能缺少依赖或权限"), "{error}");
        assert!(!error.contains("写入失败"), "不得说成写入失败: {error}");
        assert!(!error.contains("注入登录状态失败"), "{error}");
    }

    /// ③ 序列 [false, true] → Ok：证明是**轮询**，不是「启动后看一眼就下结论」。
    #[test]
    fn relaunch_polls_until_liveness_is_seen() {
        let inst = instance_with_pids(vec![4242]);
        let policy = LivenessPolicy {
            timeout: Duration::from_millis(50),
            interval: Duration::ZERO,
        };
        let mut probes = 0usize;
        let result = launch_vscode_with(
            &inst,
            policy,
            |_| Ok(()),
            || {
                probes += 1;
                probes >= 2
            },
        );
        result.expect("第二次探测到存活即应成功");
        assert_eq!(probes, 2, "必须真的探测了 2 次（轮询而非单次）");
    }

    /// ④ 启动动作失败 → 原样返回错误，且**一次都不探测**（省掉一次无意义的等待）。
    #[test]
    fn relaunch_skips_probe_when_spawn_fails() {
        let inst = instance_with_pids(vec![4242]);
        let policy = LivenessPolicy {
            timeout: Duration::ZERO,
            interval: Duration::ZERO,
        };
        let mut probes = 0usize;
        let error = launch_vscode_with(
            &inst,
            policy,
            |_| Err("重新打开 VS Code 失败: open 超时".to_string()),
            || {
                probes += 1;
                true
            },
        )
        .expect_err("启动动作失败必须返回错误");
        assert!(error.contains("open 超时"), "{error}");
        assert_eq!(probes, 0, "启动动作失败时不得探测");
    }

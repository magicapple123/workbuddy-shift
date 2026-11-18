    #[test]
    fn usage_priority_aliases_and_raw_cache_write() {
        let value = json!({
            "providerData": {
                "usage": { "inputTokens": 99, "outputTokens": 22 },
                "rawUsage": { "prompt_cache_write_tokens": 2 }
            },
            "message": { "usage": {
                "input_tokens": 10,
                "output_tokens": 3,
                "cache_read_input_tokens": 4
            }}
        });
        assert_eq!(
            usage(&value),
            Some(Usage {
                input: 10,
                output: 3,
                read: 4,
                write: 2
            })
        );
    }

    #[test]
    fn cache_write_uses_explicit_aliases_but_never_cache_miss() {
        let provider_usage_write = json!({
            "providerData": {
                "usage": {
                    "inputTokens": 99,
                    "outputTokens": 22,
                    "cache_write_input_tokens": 0,
                    "cache_creation_input_tokens": 7
                },
                "rawUsage": {
                    "prompt_cache_miss_tokens": 91,
                    "prompt_cache_write_tokens": 0
                }
            },
            "message": { "usage": {
                "input_tokens": 10,
                "output_tokens": 3,
                "cache_read_input_tokens": 4
            }}
        });
        assert_eq!(
            usage(&provider_usage_write),
            Some(Usage {
                input: 10,
                output: 3,
                read: 4,
                write: 7,
            })
        );

        let cache_miss_only = json!({
            "providerData": {
                "rawUsage": { "prompt_cache_miss_tokens": 91 }
            },
            "message": { "usage": {
                "input_tokens": 10,
                "output_tokens": 3
            }}
        });
        assert_eq!(
            usage(&cache_miss_only),
            Some(Usage {
                input: 10,
                output: 3,
                read: 0,
                write: 0,
            })
        );
    }

    #[test]
    fn cache_read_accepts_nested_provider_details() {
        let value = json!({
            "providerData": {
                "usage": {
                    "inputTokens": 99,
                    "outputTokens": 3,
                    "inputTokensDetails": [{ "cached_tokens": 7 }]
                }
            }
        });
        assert_eq!(
            usage(&value),
            Some(Usage {
                input: 99,
                output: 3,
                read: 7,
                write: 0,
            })
        );

        let raw = json!({
            "usage": {
                "prompt_tokens": 20,
                "completion_tokens": 2,
                "cache_read_input_tokens": 0,
                "prompt_cache_hit_tokens": 12
            }
        });
        assert_eq!(
            usage(&raw),
            Some(Usage {
                input: 20,
                output: 2,
                read: 12,
                write: 0,
            })
        );
    }

    #[test]
    fn source_excludes_subagents_and_counts_each_record_once() {
        let root = std::env::temp_dir().join(format!(
            "workbuddy-shift-token-stats-{}-{}",
            std::process::id(),
            crate::modules::config::now_ms()
        ));
        let project = root.join("fixture-project");
        let ignored = project.join("subagents");
        fs::create_dir_all(&ignored).expect("create fixture dirs");
        let record = json!({
            "timestamp": crate::modules::config::now_ms(),
            "providerData": {
                "model": "fixture-model",
                "usage": { "inputTokens": 20, "outputTokens": 5 }
            },
            "message": { "usage": {
                "input_tokens": 10,
                "output_tokens": 3,
                "cache_read_input_tokens": 4
            }}
        });
        fs::write(
            project.join("session.jsonl"),
            format!("{}\nnot-json\n", record),
        )
        .expect("write fixture");
        fs::write(ignored.join("agent.jsonl"), format!("{}\n", record))
            .expect("write ignored fixture");

        let result = source(root.clone(), "fixture", None, false);
        assert_eq!(result["filesScanned"], 1);
        assert_eq!(result["parseErrors"], 1);
        assert_eq!(result["summary"]["input"], 10);
        assert_eq!(result["summary"]["output"], 3);
        assert_eq!(result["summary"]["cacheRead"], 4);
        assert_eq!(result["summary"]["total"], 13);
        assert_eq!(result["summary"]["records"], 1);
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        assert_eq!(result["dailyByModel"]["fixture-model"][0]["key"], today);
        assert_eq!(result["projects"][0]["key"], "fixture-project");
        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn source_deduplicates_copied_session_history() {
        let now = crate::modules::config::now_ms();
        let root = std::env::temp_dir().join(format!(
            "workbuddy-shift-token-stats-fork-{}-{now}",
            std::process::id()
        ));
        let project = root.join("fixture-project");
        fs::create_dir_all(&project).expect("create fixture dirs");
        let record = json!({
            "timestamp": now,
            "cwd": "/fixture/example-project",
            "message": { "usage": { "input_tokens": 10, "output_tokens": 3 } }
        });
        // The copied session replays the same usage record under a new file.
        fs::write(
            project.join("session-original.jsonl"),
            format!("{}\n", record),
        )
        .expect("write original fixture");
        fs::write(
            project.join("session-forked.jsonl"),
            format!(
                "{}\n{}\n",
                record,
                json!({
                    "timestamp": now + 1_000,
                    "message": { "usage": { "input_tokens": 7, "output_tokens": 2 } }
                })
            ),
        )
        .expect("write forked fixture");
        // Back-to-back writes can land in the same millisecond, and the
        // millisecond mtime key leaves ordering to the filesystem's directory
        // iteration, which is not sorted. The copy would then be processed
        // first, own the replayed record, and leave the original with zero
        // records. Pin the mtimes so the original always precedes its copy.
        pin_mtime(
            project.join("session-original.jsonl"),
            std::time::SystemTime::now() - std::time::Duration::from_secs(60),
        );
        pin_mtime(
            project.join("session-forked.jsonl"),
            std::time::SystemTime::now(),
        );

        let result = source(root.clone(), "fixture", None, false);
        // The replayed record counts once; the fork's new record still counts.
        assert_eq!(result["summary"]["records"], 2);
        assert_eq!(result["summary"]["input"], 17);
        assert_eq!(result["summary"]["output"], 5);
        let sessions = result["sessions"].as_array().expect("session groups");
        assert_eq!(sessions.len(), 2);
        let forked = sessions
            .iter()
            .find(|session| session["sessionId"] == "session-forked")
            .expect("forked session kept its new record");
        assert_eq!(forked["input"], 7);
        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn bounded_source_excludes_records_before_cutoff() {
        let now = crate::modules::config::now_ms();
        let root = std::env::temp_dir().join(format!(
            "workbuddy-shift-token-stats-range-{}-{now}",
            std::process::id()
        ));
        let project = root.join("fixture-project");
        fs::create_dir_all(&project).expect("create fixture dirs");
        let record = |timestamp| {
            json!({
                "timestamp": timestamp,
                "cwd": "/fixture/example-project",
                "message": { "usage": { "input_tokens": 10, "output_tokens": 2 } }
            })
        };
        fs::write(
            project.join("session.jsonl"),
            format!("{}\n{}\n", record(now - 10_000), record(now - 100_000)),
        )
        .expect("write fixture");

        let result = source(root.clone(), "fixture", Some(now - 50_000), false);
        assert_eq!(result["summary"]["records"], 1);
        assert_eq!(result["summary"]["input"], 10);
        assert_eq!(result["projects"][0]["key"], "example-project");
        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn session_titles_are_file_scoped_and_independent_from_usage_cutoff() {
        let now = crate::modules::config::now_ms();
        let root = std::env::temp_dir().join(format!(
            "workbuddy-shift-token-stats-titles-{}-{now}",
            std::process::id()
        ));
        let project = root.join("fixture-project");
        fs::create_dir_all(&project).expect("create fixture dirs");
        let usage_record = |input| {
            json!({
                "timestamp": now,
                "cwd": "/private/example-project",
                "message": { "usage": { "input_tokens": input, "output_tokens": 2 } }
            })
        };

        fs::write(
            project.join("session-a.jsonl"),
            format!(
                "{}\n{}\n{}\n{}\n",
                json!({ "type": "summary", "summary": "摘要不应覆盖 AI 标题" }),
                json!({ "type": "ai-title", "aiTitle": "旧标题" }),
                usage_record(10),
                json!({ "type": "ai-title", "aiTitle": "最新 AI 标题" }),
            ),
        )
        .expect("write ai title fixture");
        fs::write(
            project.join("session-b.jsonl"),
            format!(
                "{}\n{}\n",
                json!({
                    "type": "ai-title",
                    "timestamp": now - 100_000,
                    "aiTitle": "范围外保留标题"
                }),
                usage_record(20),
            ),
        )
        .expect("write cutoff title fixture");
        fs::write(
            project.join("session-c.jsonl"),
            format!(
                "{}\n{}\n",
                usage_record(30),
                json!({ "type": "summary", "summary": "摘要回退标题" }),
            ),
        )
        .expect("write summary fixture");
        fs::write(
            project.join("session-d.jsonl"),
            format!("{}\n", usage_record(40)),
        )
        .expect("write untitled fixture");
        fs::write(
            project.join("session-e.jsonl"),
            format!(
                "{}\n{}\n",
                json!({ "type": "ai-title", "aiTitle": "最新 AI 标题" }),
                usage_record(50),
            ),
        )
        .expect("write duplicate title fixture");

        let result = source(root.clone(), "fixture", Some(now - 50_000), false);
        let sessions = result["sessions"].as_array().expect("session groups");
        let by_id = |session_id: &str| {
            sessions
                .iter()
                .find(|session| session["sessionId"] == session_id)
                .expect("session group by id")
        };

        assert_eq!(result["summary"]["input"], 150);
        assert_eq!(result["summary"]["records"], 5);
        assert_eq!(sessions.len(), 5);
        assert_eq!(by_id("session-a")["title"], "最新 AI 标题");
        assert_eq!(by_id("session-b")["title"], "范围外保留标题");
        assert_eq!(by_id("session-c")["title"], "摘要回退标题");
        assert!(by_id("session-d")["title"].is_null());
        assert_eq!(by_id("session-a")["project"], "example-project");
        assert_ne!(by_id("session-a")["key"], by_id("session-e")["key"]);

        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn request_detail_key_only_present_for_detail_sources() {
        let now = crate::modules::config::now_ms();
        let root = std::env::temp_dir().join(format!(
            "workbuddy-shift-token-stats-detail-key-{}-{now}",
            std::process::id()
        ));
        let project = root.join("fixture-project");
        fs::create_dir_all(&project).expect("create fixture dirs");
        fs::write(
            project.join("session.jsonl"),
            format!(
                "{}\n",
                json!({
                    "timestamp": now,
                    "message": { "usage": { "input_tokens": 10, "output_tokens": 3 } }
                })
            ),
        )
        .expect("write fixture");

        let aggregate_only = source(root.clone(), "workbuddy", None, false);
        assert!(aggregate_only.get("requests").is_none());

        let detailed = source(root.clone(), "codebuddy-cli", None, true);
        let rows = detailed["requests"].as_array().expect("request rows");
        assert_eq!(rows.len(), 1);
        // 明细只是附加键，聚合数值与不采集明细时完全一致。
        assert_eq!(detailed["summary"], aggregate_only["summary"]);

        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn request_detail_rows_share_the_total_formula_and_sort_newest_first() {
        let now = crate::modules::config::now_ms();
        let root = std::env::temp_dir().join(format!(
            "workbuddy-shift-token-stats-detail-rows-{}-{now}",
            std::process::id()
        ));
        let project = root.join("fixture-project");
        fs::create_dir_all(&project).expect("create fixture dirs");
        let record = |timestamp, input, write| {
            json!({
                "timestamp": timestamp,
                "cwd": "/private/example-project",
                "providerData": { "model": "fixture-model" },
                "message": { "usage": {
                    "input_tokens": input,
                    "output_tokens": 2,
                    "cache_read_input_tokens": 5,
                    "cache_write_input_tokens": write
                }}
            })
        };
        // 文件顺序刻意与时间顺序相反，排序必须来自 timestamp。
        fs::write(
            project.join("session-detail.jsonl"),
            format!(
                "{}\n{}\n{}\n",
                record(now - 1_000, 10, 1),
                record(now - 2_000, 20, 2),
                record(now - 3_000, 30, 3),
            ),
        )
        .expect("write fixture");

        let result = source(root.clone(), "codebuddy-cli", None, true);
        assert_eq!(result["summary"]["records"], 3);
        // input 60 + output 6 + cacheWrite 6；cacheRead 不计入合计。
        assert_eq!(result["summary"]["total"], 72);
        let rows = result["requests"].as_array().expect("request rows");
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["timestamp"], now - 1_000);
        assert_eq!(rows[2]["timestamp"], now - 3_000);
        assert_eq!(rows[0]["model"], "fixture-model");
        // 只暴露 cwd 的 basename，不返回绝对路径。
        assert_eq!(rows[0]["project"], "example-project");
        assert_eq!(rows[0]["sessionId"], "session-detail");
        assert!(rows[0]["title"].is_null());
        assert_eq!(rows[0]["input"], 10);
        assert_eq!(rows[0]["output"], 2);
        assert_eq!(rows[0]["cacheRead"], 5);
        assert_eq!(rows[0]["cacheWrite"], 1);
        assert_eq!(rows[0]["total"], 13);
        // 缓存未命中 = input - cacheRead，与 `summary` 同口径。
        assert_eq!(rows[0]["uncachedInput"], 5);
        // 无 providerData.rawUsage 时思考过程为 0。
        assert_eq!(rows[0]["thinking"], 0);
        let row_total: u64 = rows
            .iter()
            .map(|row| row["total"].as_u64().unwrap_or(0))
            .sum();
        assert_eq!(row_total, result["summary"]["total"].as_u64().unwrap_or(0));
        let row_uncached: u64 = rows
            .iter()
            .map(|row| row["uncachedInput"].as_u64().unwrap_or(0))
            .sum();
        assert_eq!(
            row_uncached,
            result["summary"]["uncachedInput"].as_u64().unwrap_or(0)
        );

        fs::remove_dir_all(root).expect("remove fixture");
    }

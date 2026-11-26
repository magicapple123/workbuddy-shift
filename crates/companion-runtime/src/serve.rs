//! 常驻运行时：RPC、Codeg webhook 与通知轮询的 serve 循环。

use super::integrations::{self, install_hooks};
use super::{collector_request_command, ensure_hook_binary, ingest_hook_durably, Notifications};
use companion_core::{adapters::Collector, atomic_json, now, text};
use companion_runtime::{endpoint, home, HookSpool, PROTOCOL};
use fs2::FileExt;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::Read;
use std::time::{Duration, Instant};

pub(super) fn serve() -> Result<(), String> {
    let home = home();
    let dir = home.join(".workbuddy-shift-companion");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join("runtime-v1.lock"))
        .map_err(|e| e.to_string())?;
    if lock.try_lock_exclusive().is_err() {
        return Ok(());
    }
    // Built-in hook install only copies this binary when that agent's config
    // directory already exists. Custom commands point at the same path, so
    // publish it on every start or a custom-only setup has nothing to run.
    let _ = ensure_hook_binary(&home);
    let mut collector = Collector::new(home.clone())?;
    let warning = install_hooks(&collector).err();
    let server = tiny_http::Server::http("127.0.0.1:0").map_err(|e| e.to_string())?;
    let port = server.server_addr().to_ip().ok_or("无效监听地址")?.port();
    let token = uuid::Uuid::new_v4().to_string();
    let codeg_path = format!("/api/codeg-webhook/{}", uuid::Uuid::new_v4());
    collector.configure_codeg_webhook(format!("http://127.0.0.1:{port}{codeg_path}"));
    type Job = (
        String,
        Value,
        Option<std::sync::mpsc::Sender<Result<Value, String>>>,
    );
    let (jobs, receiver) = std::sync::mpsc::channel::<Job>();
    let stream_jobs = jobs.clone();
    collector.set_codeg_stream_sink(std::sync::Arc::new(move |frame| {
        let _ = stream_jobs.send(("codeg_stream".into(), frame, None));
    }));
    let (updates, changes) = std::sync::mpsc::channel::<Value>();
    let initial_settings = collector.settings.clone();
    let codeg_integrated = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
        collector.integration_automatic("codeg"),
    ));
    let worker_codeg_integrated = codeg_integrated.clone();
    // WorkBuddy / CodeBuddy IDE hooks write to this spool instead of blocking on
    // RPC. The worker drains it every iteration (~100ms), including events that
    // accumulated while the daemon was not running.
    let mut hook_spool = HookSpool::new(&home);
    let worker = std::thread::spawn(move || {
        let mut poll_at = Instant::now() - Duration::from_secs(5);
        loop {
            let _ = hook_spool.drain(|payload| {
                if ingest_hook_durably(&mut collector, &payload)
                    .is_ok_and(|value| value == json!(true))
                {
                    let _ = updates.send(collector.hub.snapshot());
                }
            });
            if poll_at.elapsed() >= Duration::from_secs(2) {
                collector.poll();
                poll_at = Instant::now();
                let snapshot = collector.hub.snapshot();
                let _ = updates.send(snapshot);
            }
            match receiver.recv_timeout(Duration::from_millis(100)) {
                Ok((command, payload, reply)) => {
                    if command == "shutdown" {
                        break;
                    }
                    let result = if command == "codeg_stream" {
                        Ok(json!(collector.ingest_codeg_stream(&payload)))
                    } else if command == "hook" {
                        ingest_hook_durably(&mut collector, &payload)
                    } else if command == "integrations_get" {
                        Ok(integrations::get(&collector))
                    } else if command == "integrations_set" {
                        {
                            let result = integrations::set(&mut collector, &payload);
                            // Policy is durable even when remote registration fails.
                            worker_codeg_integrated.store(
                                collector.integration_automatic("codeg"),
                                std::sync::atomic::Ordering::Release,
                            );
                            result
                        }
                    } else {
                        collector.request(&command, &payload)
                    };
                    if command == "settings_set" && result.is_ok() {
                        let _ = install_hooks(&collector);
                        poll_at = Instant::now() - Duration::from_secs(5);
                    }
                    if command != "codeg_stream" || result.as_ref().is_ok_and(|v| v == &json!(true))
                    {
                        let _ = updates.send(collector.hub.snapshot());
                    }
                    if let Some(reply) = reply {
                        let _ = reply.send(result);
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => {
                    break;
                }
            }
        }
        collector.stop_codeg_webhook();
    });
    atomic_json(
        &endpoint(&home),
        &json!({"protocol":PROTOCOL,"port":port,"token":token,"pid":std::process::id()}),
    )?;
    let mut clients: HashMap<String, Instant> = HashMap::new();
    let mut owner = String::new();
    let mut idle = Instant::now();
    let saved: Value = std::fs::read(dir.join("desktop-notifications.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or(Value::Null);
    let mut notify = Notifications {
        seen: saved["seen"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        initialized: false,
    };
    let mut alerts = Vec::new();
    let mut snapshot =
        json!({"version":1,"ready":false,"sessions":[],"events":[],"sources":{},"ts":now()});
    let mut settings = initial_settings;
    let mut last_seen = String::new();
    loop {
        while let Ok(next) = changes.try_recv() {
            snapshot = next;
            alerts.extend(notify.ingest(&snapshot));
            if alerts.len() > 60 {
                alerts.drain(..alerts.len() - 60);
            }
            let seen = json!(notify.seen).to_string();
            if seen != last_seen
                && atomic_json(
                    &dir.join("desktop-notifications.json"),
                    &json!({"seen":notify.seen,"alerts":[]}),
                )
                .is_ok()
            {
                last_seen = seen;
            }
        }
        clients.retain(|_, t| t.elapsed() < Duration::from_secs(15));
        if !clients.contains_key(&owner) {
            owner = clients.keys().min().cloned().unwrap_or_default();
        }
        if !clients.is_empty() {
            idle = Instant::now();
        }
        if idle.elapsed() > Duration::from_secs(20) {
            break;
        }
        let Some(mut request) = server
            .recv_timeout(Duration::from_millis(100))
            .map_err(|e| e.to_string())?
        else {
            continue;
        };
        // Codeg cannot supply Authorization headers; a per-runtime random URL
        // is the callback capability. It is never accepted by the RPC route.
        if request.url() == codeg_path
            && request.method() == &tiny_http::Method::Post
            && !request.headers().iter().any(|h| h.field.equiv("Origin"))
        {
            if settings["sources"]["codeg"]["enabled"] != true
                || !codeg_integrated.load(std::sync::atomic::Ordering::Acquire)
            {
                let _ = request.respond(tiny_http::Response::empty(410));
                continue;
            }
            let mut body = Vec::new();
            let _ = request.as_reader().take(65537).read_to_end(&mut body);
            let status = if body.len() > 65536 {
                413
            } else {
                match serde_json::from_slice::<Value>(&body) {
                    Ok(p)
                        if p["source"] == "codeg"
                            && p["connection_id"].is_string()
                            && companion_core::adapters::CODEG_EVENTS
                                .contains(&text(&p["event"]).as_str()) =>
                    {
                        if jobs.send(("hook".into(), p, None)).is_ok() {
                            204
                        } else {
                            503
                        }
                    }
                    _ => 400,
                }
            };
            let _ = request.respond(tiny_http::Response::empty(status));
            continue;
        }
        let authenticated = request.headers().iter().any(|h| {
            h.field.equiv("Authorization") && h.value.as_str() == format!("Bearer {token}")
        });
        if request.url() != "/rpc"
            || request.method() != &tiny_http::Method::Post
            || !authenticated
            || request.headers().iter().any(|h| h.field.equiv("Origin"))
        {
            let _ = request.respond(tiny_http::Response::empty(403));
            continue;
        }
        let mut bytes = Vec::new();
        let _ = request
            .as_reader()
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes);
        if bytes.len() > 1024 * 1024 {
            let _ = request.respond(tiny_http::Response::empty(413));
            continue;
        }
        let result = (|| -> Result<Value, String> {
            let req: Value = serde_json::from_slice(&bytes).map_err(|_| "请求格式无效")?;
            let p = &req["payload"];
            match req["command"].as_str().unwrap_or("") {
                "hello" => {
                    let id = p["client"]
                        .as_str()
                        .filter(|s| !s.is_empty() && s.len() < 100)
                        .ok_or("客户端无效")?;
                    clients.insert(id.into(), Instant::now());
                    if owner.is_empty() {
                        owner = id.into();
                    }
                    Ok(json!({"protocol":PROTOCOL}))
                }
                "poll" => {
                    let id = p["client"].as_str().ok_or("客户端无效")?;
                    let entry = clients.get_mut(id).ok_or("客户端租约失效")?;
                    *entry = Instant::now();
                    let native = if owner == id {
                        std::mem::take(&mut alerts)
                            .into_iter()
                            .filter(|a| {
                                settings["notifications"]["desktop"] == true
                                    && settings["notifications"][text(&a["kind"])] != false
                            })
                            .map(|mut a| {
                                a["sound"] = settings["notifications"]["sound"].clone();
                                a
                            })
                            .collect::<Vec<_>>()
                    } else {
                        vec![]
                    };
                    Ok(
                        json!({"snapshot":snapshot,"settings":settings,"notifications":native,"owner":owner==id,"warning":warning}),
                    )
                }
                "leave" => {
                    clients.remove(&text(&p["client"]));
                    Ok(json!(true))
                }
                "hook" => {
                    let terminal = p["agent_source"] == "codex";
                    if terminal {
                        let (send, receive) = std::sync::mpsc::channel();
                        jobs.send(("hook".into(), p.clone(), Some(send)))
                            .map_err(|_| "采集器已退出")?;
                        receive
                            .recv_timeout(Duration::from_secs(2))
                            .map_err(|_| "结束事件未被采集器确认")?
                    } else {
                        jobs.send(("hook".into(), p.clone(), None))
                            .map_err(|_| "采集器已退出")?;
                        Ok(json!(true))
                    }
                }
                "settings_get" => Ok(settings.clone()),
                command if collector_request_command(command) => {
                    let (send, receive) = std::sync::mpsc::channel();
                    jobs.send((command.into(), p.clone(), Some(send)))
                        .map_err(|_| "采集器已退出")?;
                    let value = receive
                        .recv_timeout(Duration::from_secs(10))
                        .map_err(|_| "采集器忙，请重试")??;
                    if command == "settings_set" {
                        settings = value.clone();
                    }
                    Ok(value)
                }
                _ => Err("不支持的命令".into()),
            }
        })();
        let body = match result {
            Ok(v) => json!({"value":v}),
            Err(e) => json!({"error":e}),
        };
        let _ = request.respond(
            tiny_http::Response::from_string(body.to_string()).with_header(
                tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap(),
            ),
        );
    }
    let _ = jobs.send(("shutdown".into(), Value::Null, None));
    drop(jobs);
    let _ = worker.join();
    let _ = std::fs::remove_file(endpoint(&home));
    drop(lock);
    Ok(())
}

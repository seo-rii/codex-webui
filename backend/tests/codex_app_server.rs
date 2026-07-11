use std::{collections::HashMap, fs, path::PathBuf, time::Duration};

use anyhow::{Context, Result};
use backend::codex_app_server::{
    AppServerClient, AppServerClientConfig, AppServerManager, AppServerNotification,
    AppServerProfile, AppServerRequest, app_server_request_timed_out, app_server_timeout_recovered,
};
use serde_json::{Value, json};
#[cfg(target_os = "linux")]
use sha2::{Digest, Sha256};
use tokio::{sync::broadcast, time::timeout};
use uuid::Uuid;

fn temp_dir(label: &str) -> Result<PathBuf> {
    let path = std::env::temp_dir().join(format!("codex-webui-{label}-{}", Uuid::new_v4()));
    fs::create_dir_all(&path).with_context(|| format!("failed to create {}", path.display()))?;
    Ok(path)
}

#[cfg(unix)]
fn write_executable(path: &std::path::Path, contents: &str) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::write(path, contents)?;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

async fn recv_notification(
    receiver: &mut broadcast::Receiver<AppServerNotification>,
    method: &str,
) -> Result<AppServerNotification> {
    loop {
        let event = timeout(Duration::from_secs(2), receiver.recv())
            .await
            .context("timed out waiting for notification")??;
        if event.method == method {
            return Ok(event);
        }
    }
}

async fn recv_request(
    receiver: &mut broadcast::Receiver<AppServerRequest>,
    method: &str,
) -> Result<AppServerRequest> {
    loop {
        let event = timeout(Duration::from_secs(2), receiver.recv())
            .await
            .context("timed out waiting for server request")??;
        if event.method == method {
            return Ok(event);
        }
    }
}

fn test_client_config() -> AppServerClientConfig {
    AppServerClientConfig {
        codex_bin: env!("CARGO_BIN_EXE_fake_codex_app_server").to_string(),
        ..AppServerClientConfig::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_handles_requests_notifications_and_server_requests() -> Result<()> {
    let codex_home = temp_dir("client-codex-home")?;
    let stderr_log_path = codex_home.join("stderr.log");
    let client = AppServerClient::new(
        AppServerProfile {
            id: "default".to_string(),
            codex_home: codex_home.clone(),
        },
        AppServerClientConfig {
            stderr_log_path: Some(stderr_log_path),
            ..test_client_config()
        },
    );
    let mut notifications = client.subscribe_notifications();
    let mut requests = client.subscribe_requests();

    let echo = client.request("echo", json!({ "value": 42 })).await?;
    assert_eq!(echo, json!({ "value": 42 }));

    let ready = recv_notification(&mut notifications, "fake/ready").await?;
    assert_eq!(
        ready.params.get("codexHome").and_then(Value::as_str),
        Some(codex_home.to_string_lossy().as_ref())
    );

    let emitted = client
        .request("emitNotification", json!({ "kind": "ping" }))
        .await?;
    assert_eq!(emitted, json!({ "ok": true }));

    let custom = recv_notification(&mut notifications, "fake/custom").await?;
    assert_eq!(custom.params, json!({ "kind": "ping" }));

    let asked = client.request("askQuestion", json!({})).await?;
    assert_eq!(asked, json!({ "ok": true }));

    let server_request = recv_request(&mut requests, "input/request").await?;
    assert_eq!(server_request.params, json!({ "question": "Continue?" }));

    client
        .respond(server_request.id.clone(), json!({ "answer": "yes" }))
        .await?;

    let resolved = recv_notification(&mut notifications, "fake/serverRequestResolved").await?;
    assert_eq!(resolved.params.get("id"), Some(&server_request.id));
    assert_eq!(
        resolved.params.get("result"),
        Some(&json!({ "answer": "yes" }))
    );

    client.close().await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manager_reuses_one_process_per_profile() -> Result<()> {
    let temp = temp_dir("manager")?;
    let start_log = temp.join("starts.log");
    let profile_a_home = temp.join("profile-a");
    let profile_b_home = temp.join("profile-b");
    fs::create_dir_all(&profile_a_home)?;
    fs::create_dir_all(&profile_b_home)?;

    let mut extra_env = HashMap::new();
    extra_env.insert(
        "FAKE_CODEX_START_LOG".to_string(),
        start_log.to_string_lossy().to_string(),
    );

    let manager = AppServerManager::new(AppServerClientConfig {
        extra_env,
        ..test_client_config()
    });

    let client_a1 = manager
        .get_or_create(AppServerProfile {
            id: "profile-a".to_string(),
            codex_home: profile_a_home,
        })
        .await;
    client_a1
        .request("echo", json!({ "profile": "a1" }))
        .await?;

    let client_a2 = manager
        .get_or_create(AppServerProfile {
            id: "profile-a".to_string(),
            codex_home: temp.join("ignored-profile-a"),
        })
        .await;
    client_a2
        .request("echo", json!({ "profile": "a2" }))
        .await?;

    let client_b = manager
        .get_or_create(AppServerProfile {
            id: "profile-b".to_string(),
            codex_home: profile_b_home,
        })
        .await;
    client_b.request("echo", json!({ "profile": "b" })).await?;

    manager.close_all().await?;

    let starts = fs::read_to_string(&start_log)
        .with_context(|| format!("failed to read {}", start_log.display()))?;
    assert_eq!(
        starts.lines().count(),
        2,
        "expected one process per profile"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn manager_reclaims_idle_process_when_cap_is_full() -> Result<()> {
    let temp = temp_dir("manager-idle-eviction")?;
    let first_home = temp.join("first");
    let second_home = temp.join("second");
    fs::create_dir_all(&first_home)?;
    fs::create_dir_all(&second_home)?;
    let manager = AppServerManager::new(AppServerClientConfig {
        max_processes: 1,
        idle_client_timeout: Duration::ZERO,
        startup_timeout: Duration::from_secs(1),
        request_timeout: Duration::from_secs(1),
        ..test_client_config()
    });
    let first = manager
        .get_or_create_with_key(
            "first::session::one".to_string(),
            AppServerProfile {
                id: "first".to_string(),
                codex_home: first_home,
            },
        )
        .await;
    first.request("echo", json!({ "client": "first" })).await?;

    let second = manager
        .get_or_create_with_key(
            "second::session::two".to_string(),
            AppServerProfile {
                id: "second".to_string(),
                codex_home: second_home,
            },
        )
        .await;
    assert_eq!(
        second
            .request("echo", json!({ "client": "second" }))
            .await?,
        json!({ "client": "second" })
    );
    assert_eq!(manager.client_count().await, 1);

    manager.close_all().await?;
    Ok(())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn maintenance_timeout_does_not_interrupt_concurrent_request() -> Result<()> {
    let temp = temp_dir("maintenance-timeout")?;
    let script_path = temp.join("fake-codex.py");
    let start_log = temp.join("starts.log");
    write_executable(
        &script_path,
        r#"#!/usr/bin/env python3
import json
import os
import sys

with open(os.environ["FAKE_CODEX_START_LOG"], "a", encoding="utf-8") as log:
    log.write(f"{os.getpid()}\n")

for raw_line in sys.stdin:
    payload = json.loads(raw_line)
    method = payload.get("method")
    if method == "initialize":
        result = {"serverInfo": {"name": "fake"}}
    elif method == "initialized":
        continue
    elif method == "thread/read":
        continue
    else:
        result = payload.get("params") or {}
    print(json.dumps({"id": payload.get("id"), "result": result}), flush=True)
"#,
    )?;

    let profile = AppServerProfile {
        id: "default".to_string(),
        codex_home: temp.join("codex-home"),
    };
    let manager = AppServerManager::new(AppServerClientConfig {
        codex_bin: script_path.display().to_string(),
        request_timeout: Duration::from_secs(2),
        startup_timeout: Duration::from_secs(2),
        extra_env: HashMap::from([(
            "FAKE_CODEX_START_LOG".to_string(),
            start_log.display().to_string(),
        )]),
        ..AppServerClientConfig::default()
    });
    let client = manager.get_or_create(profile.clone()).await;
    let maintenance_client = client.clone();
    let maintenance = tokio::spawn(async move {
        maintenance_client
            .request_with_timeout(
                "thread/read",
                json!({ "threadId": "thread-1" }),
                Duration::from_millis(200),
                false,
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    assert_eq!(
        client
            .request("thread/resume", json!({ "threadId": "thread-1" }))
            .await?,
        json!({ "threadId": "thread-1" })
    );
    let timeout_error = maintenance
        .await
        .context("maintenance request task failed")?
        .expect_err("thread/read maintenance request should time out");
    assert!(app_server_request_timed_out(&timeout_error));
    assert!(!app_server_timeout_recovered(&timeout_error));

    let snapshots = manager.process_snapshots_for_profiles(vec![profile]).await;
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].pending_request_count, 0);
    assert_eq!(fs::read_to_string(&start_log)?.lines().count(), 1);

    manager.close_all().await?;
    Ok(())
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn write_timeout_cleans_pending_and_explicit_recovery_restarts() -> Result<()> {
    let temp = temp_dir("writer-timeout")?;
    let script_path = temp.join("fake-codex.py");
    let start_log = temp.join("starts.log");
    let blocked_once = temp.join("blocked-once");
    write_executable(
        &script_path,
        r#"#!/usr/bin/env python3
import json
import os
import sys
import time

with open(os.environ["FAKE_CODEX_START_LOG"], "a", encoding="utf-8") as log:
    log.write(f"{os.getpid()}\n")

blocked_once = os.environ["FAKE_CODEX_BLOCKED_ONCE"]
for raw_line in sys.stdin:
    payload = json.loads(raw_line)
    method = payload.get("method")
    if method == "initialize":
        result = {"serverInfo": {"name": "fake"}}
    elif method == "initialized":
        continue
    else:
        result = payload.get("params") or {}
    print(json.dumps({"id": payload.get("id"), "result": result}), flush=True)
    if method == "stopReading" and not os.path.exists(blocked_once):
        open(blocked_once, "w", encoding="utf-8").close()
        time.sleep(60)
"#,
    )?;

    let profile = AppServerProfile {
        id: "default".to_string(),
        codex_home: temp.join("codex-home"),
    };
    let manager = AppServerManager::new(AppServerClientConfig {
        codex_bin: script_path.display().to_string(),
        request_timeout: Duration::from_secs(2),
        startup_timeout: Duration::from_secs(2),
        extra_env: HashMap::from([
            (
                "FAKE_CODEX_START_LOG".to_string(),
                start_log.display().to_string(),
            ),
            (
                "FAKE_CODEX_BLOCKED_ONCE".to_string(),
                blocked_once.display().to_string(),
            ),
        ]),
        ..AppServerClientConfig::default()
    });
    let client = manager.get_or_create(profile.clone()).await;
    client.request("stopReading", json!({})).await?;

    let timeout_error = client
        .request_with_timeout(
            "thread/read",
            json!({ "padding": "x".repeat(8 * 1024 * 1024) }),
            Duration::from_millis(200),
            false,
        )
        .await
        .expect_err("blocked app-server writer should time out");
    assert!(app_server_request_timed_out(&timeout_error));
    assert!(!app_server_timeout_recovered(&timeout_error));
    let snapshots = manager
        .process_snapshots_for_profiles(vec![profile.clone()])
        .await;
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].pending_request_count, 0);

    let recovery_error = client
        .request_with_timeout("echo", json!({}), Duration::from_millis(200), true)
        .await
        .expect_err("explicit recovery request should time out on the blocked writer");
    assert!(app_server_timeout_recovered(&recovery_error));
    assert_eq!(
        client.request("echo", json!({ "restarted": true })).await?,
        json!({ "restarted": true })
    );
    assert_eq!(fs::read_to_string(&start_log)?.lines().count(), 2);

    manager.close_all().await?;
    Ok(())
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn incompatible_handoff_daemon_is_not_manager_active() -> Result<()> {
    use std::os::unix::net::UnixListener;

    let temp = temp_dir("incompatible-handoff")?;
    let handoff_dir = temp.join("handoff");
    fs::create_dir_all(&handoff_dir)?;
    let profile = AppServerProfile {
        id: "default".to_string(),
        codex_home: temp.join("codex-home"),
    };
    let config = AppServerClientConfig {
        handoff_dir: Some(handoff_dir.clone()),
        ..test_client_config()
    };

    let mut hasher = Sha256::new();
    hasher.update(profile.id.as_bytes());
    hasher.update(b"\0");
    hasher.update(profile.id.as_bytes());
    hasher.update(b"\0");
    hasher.update(profile.codex_home.display().to_string().as_bytes());
    hasher.update(b"\0");
    hasher.update(handoff_dir.display().to_string().as_bytes());
    let suffix = hasher
        .finalize()
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let socket_path = std::env::temp_dir()
        .join("codex-webui-app-server")
        .join(format!("{suffix}.sock"));
    fs::create_dir_all(socket_path.parent().context("socket path has no parent")?)?;
    let _ = fs::remove_file(&socket_path);
    let listener = UnixListener::bind(&socket_path)?;

    let proc_stat = fs::read_to_string(format!("/proc/{}/stat", std::process::id()))?;
    let fields = proc_stat
        .rsplit_once(") ")
        .context("unexpected /proc stat format")?
        .1
        .split_whitespace()
        .collect::<Vec<_>>();
    let process_group_id = fields[2].parse::<u32>()?;
    let start_time_ticks = fields[19].parse::<u64>()?;
    let meta_path = handoff_dir.join(format!("default-{suffix}.json"));
    fs::write(
        &meta_path,
        serde_json::to_vec(&json!({
            "pid": std::process::id(),
            "process_identity": {
                "pid": std::process::id(),
                "process_group_id": process_group_id,
                "start_time_ticks": start_time_ticks
            },
            "enabled_features": ["goals"],
            "client_key": profile.id.clone(),
            "profile_id": profile.id.clone(),
            "socket_path": socket_path.display().to_string(),
            "codex_bin": config.codex_bin.clone(),
            "codex_home": profile.codex_home.display().to_string(),
            "started_at_ms": 1
        }))?,
    )?;

    let manager = AppServerManager::new(config);
    assert!(!manager.profile_has_active_process("default").await);
    assert_eq!(manager.active_process_count().await, 0);
    assert!(
        manager
            .process_snapshots_for_profiles(vec![profile])
            .await
            .is_empty()
    );

    drop(listener);
    let _ = fs::remove_file(socket_path);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_supports_account_requests_and_notifications() -> Result<()> {
    let codex_home = temp_dir("account-codex-home")?;
    let client = AppServerClient::new(
        AppServerProfile {
            id: "default".to_string(),
            codex_home,
        },
        test_client_config(),
    );
    let mut notifications = client.subscribe_notifications();

    let account = client
        .request("account/read", json!({ "refreshToken": false }))
        .await?;
    assert_eq!(
        account,
        json!({
            "account": {
                "type": "chatgpt",
                "email": "demo@example.com",
                "planType": "plus"
            },
            "requiresOpenaiAuth": false
        })
    );

    let login = client
        .request("account/login/start", json!({ "type": "chatgpt" }))
        .await?;
    assert_eq!(
        login,
        json!({
            "type": "chatgpt",
            "loginId": "login-chatgpt-1",
            "authUrl": "https://example.com/auth"
        })
    );

    let login_completed = recv_notification(&mut notifications, "account/login/completed").await?;
    assert_eq!(
        login_completed.params,
        json!({
            "loginId": "login-chatgpt-1",
            "success": true,
            "error": Value::Null
        })
    );

    let account_updated = recv_notification(&mut notifications, "account/updated").await?;
    assert_eq!(account_updated.params, json!({ "type": "chatgpt" }));

    let rate_limits = recv_notification(&mut notifications, "account/rateLimits/updated").await?;
    assert_eq!(rate_limits.params, json!({ "source": "fake" }));

    let canceled = client
        .request(
            "account/login/cancel",
            json!({ "loginId": "login-chatgpt-1" }),
        )
        .await?;
    assert_eq!(
        canceled,
        json!({
            "status": "canceled",
            "loginId": "login-chatgpt-1"
        })
    );

    let logout = client.request("account/logout", json!({})).await?;
    assert_eq!(logout, json!({ "ok": true }));

    let logout_updated = recv_notification(&mut notifications, "account/updated").await?;
    assert_eq!(logout_updated.params, json!({ "type": Value::Null }));

    client.close().await?;
    Ok(())
}

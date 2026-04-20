use std::{collections::HashMap, fs, path::PathBuf, time::Duration};

use anyhow::{Context, Result};
use backend::codex_app_server::{
    AppServerClient, AppServerClientConfig, AppServerManager, AppServerNotification,
    AppServerProfile, AppServerRequest,
};
use serde_json::{Value, json};
use tokio::{sync::broadcast, time::timeout};
use uuid::Uuid;

fn temp_dir(label: &str) -> Result<PathBuf> {
    let path = std::env::temp_dir().join(format!("codex-webui-{label}-{}", Uuid::new_v4()));
    fs::create_dir_all(&path).with_context(|| format!("failed to create {}", path.display()))?;
    Ok(path)
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

use std::{
    env,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

fn main() {
    println!("cargo:rerun-if-env-changed=CODEX_WEBUI_PACKAGE_VERSION");
    println!("cargo:rerun-if-env-changed=CODEX_WEBUI_BUILD_VERSION");
    println!("cargo:rerun-if-env-changed=CODEX_WEBUI_BUILD_COMMIT");
    println!("cargo:rerun-if-env-changed=CODEX_WEBUI_BUILD_COMMIT_SHORT");
    println!("cargo:rerun-if-env-changed=CODEX_WEBUI_BUILD_DIRTY");
    println!("cargo:rerun-if-env-changed=CODEX_WEBUI_BUILD_EPOCH_MS");
    println!("cargo:rerun-if-env-changed=CODEX_WEBUI_BUILD_TIMESTAMP");
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/refs");
    println!("cargo:rerun-if-changed=../.git/packed-refs");

    let package_version = non_empty_env("CODEX_WEBUI_PACKAGE_VERSION")
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    let commit = non_empty_env("CODEX_WEBUI_BUILD_COMMIT")
        .or_else(|| git_output(&["rev-parse", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_string());
    let commit_short =
        non_empty_env("CODEX_WEBUI_BUILD_COMMIT_SHORT").unwrap_or_else(|| short_commit(&commit));
    let dirty = non_empty_env("CODEX_WEBUI_BUILD_DIRTY").unwrap_or_else(|| {
        git_dirty()
            .map(|value| value.to_string())
            .unwrap_or_else(|| "false".to_string())
    });
    let epoch_ms = non_empty_env("CODEX_WEBUI_BUILD_EPOCH_MS").unwrap_or_else(current_epoch_ms);
    let timestamp =
        non_empty_env("CODEX_WEBUI_BUILD_TIMESTAMP").unwrap_or_else(|| epoch_ms.clone());
    let requested_version = non_empty_env("CODEX_WEBUI_BUILD_VERSION");
    let version = if requested_version
        .as_ref()
        .is_some_and(|value| value.contains(&commit_short))
    {
        requested_version.unwrap()
    } else {
        let prefix = requested_version.unwrap_or_else(|| package_version.clone());
        format!(
            "{}-{}{}-{}",
            prefix,
            commit_short,
            if dirty == "true" { "-dirty" } else { "" },
            epoch_ms
        )
    };

    println!("cargo:rustc-env=CODEX_WEBUI_PACKAGE_VERSION={package_version}");
    println!("cargo:rustc-env=CODEX_WEBUI_BUILD_VERSION={version}");
    println!("cargo:rustc-env=CODEX_WEBUI_BUILD_COMMIT={commit}");
    println!("cargo:rustc-env=CODEX_WEBUI_BUILD_COMMIT_SHORT={commit_short}");
    println!("cargo:rustc-env=CODEX_WEBUI_BUILD_DIRTY={dirty}");
    println!("cargo:rustc-env=CODEX_WEBUI_BUILD_EPOCH_MS={epoch_ms}");
    println!("cargo:rustc-env=CODEX_WEBUI_BUILD_TIMESTAMP={timestamp}");
}

fn non_empty_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn git_output(args: &[&str]) -> Option<String> {
    let root = env::var("CARGO_MANIFEST_DIR")
        .ok()
        .map(|value| format!("{value}/.."))?;
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn git_dirty() -> Option<bool> {
    git_output(&["status", "--porcelain"]).map(|value| !value.is_empty())
}

fn short_commit(commit: &str) -> String {
    let trimmed = commit.trim();
    if trimmed.is_empty() || trimmed == "unknown" {
        "unknown".to_string()
    } else {
        trimmed.chars().take(12).collect()
    }
}

fn current_epoch_ms() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

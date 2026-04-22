use super::*;

pub(crate) async fn append_audit_log(config: &Config, entry: AuditLogEntry) -> Result<()> {
    tokio_fs::create_dir_all(&config.data_dir)
        .await
        .context("failed to create data directory")?;
    let path = config.data_dir.join("audit-log.jsonl");
    let mut file = tokio_fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
        .context("failed to open audit log")?;
    let line = serde_json::to_string(&entry).context("failed to serialize audit log entry")?;
    file.write_all(line.as_bytes())
        .await
        .context("failed to write audit log entry")?;
    file.write_all(b"\n")
        .await
        .context("failed to finalize audit log entry")?;
    Ok(())
}

pub(crate) async fn list_audit_log(config: &Config, limit: usize) -> Result<Value> {
    let path = config.data_dir.join("audit-log.jsonl");
    let raw = match tokio_fs::read_to_string(path).await {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error).context("failed to read audit log"),
    };

    let mut entries = raw
        .lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<AuditLogEntry>(line).ok())
        .take(limit.max(1))
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| right.at.cmp(&left.at));

    Ok(json!({ "entries": entries }))
}

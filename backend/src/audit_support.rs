use super::*;

pub(crate) async fn append_audit_log(config: &Config, entry: AuditLogEntry) -> Result<()> {
    tokio_fs::create_dir_all(&config.data_dir)
        .await
        .context("failed to create data directory")?;
    let path = config.data_dir.join("audit-log.jsonl");
    if tokio_fs::metadata(&path)
        .await
        .is_ok_and(|metadata| metadata.len() > AUDIT_LOG_ROTATE_BYTES)
    {
        let rotated_path = config.data_dir.join("audit-log.jsonl.1");
        let _ = tokio_fs::remove_file(&rotated_path).await;
        let _ = tokio_fs::rename(&path, rotated_path).await;
    }
    let mut file = tokio_fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
        .context("failed to open audit log")?;
    let line = serde_json::to_string(&entry).context("failed to serialize audit log entry")?;
    let mut record = Vec::with_capacity(line.len() + 1);
    record.extend_from_slice(line.as_bytes());
    record.push(b'\n');
    file.write_all(&record)
        .await
        .context("failed to write audit log entry")?;
    Ok(())
}

pub(crate) async fn list_audit_log(config: &Config, limit: usize) -> Result<Value> {
    let path = config.data_dir.join("audit-log.jsonl");
    let clamped_limit = limit.clamp(1, MAX_AUDIT_LOG_LIMIT);
    let mut file = match tokio_fs::File::open(&path).await {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(json!({ "entries": [] }));
        }
        Err(error) => return Err(error).context("failed to open audit log"),
    };
    let file_len = file
        .metadata()
        .await
        .context("failed to inspect audit log")?
        .len();
    let start = file_len.saturating_sub(AUDIT_LOG_TAIL_READ_BYTES);
    file.seek(std::io::SeekFrom::Start(start))
        .await
        .context("failed to seek audit log")?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .await
        .context("failed to read audit log tail")?;

    let raw = String::from_utf8_lossy(&bytes);
    let mut lines = raw.lines();
    if start > 0 {
        let _ = lines.next();
    }
    let mut entries = lines
        .rev()
        .filter_map(|line| serde_json::from_str::<AuditLogEntry>(line).ok())
        .take(clamped_limit)
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| right.at.cmp(&left.at));

    Ok(json!({ "entries": entries }))
}

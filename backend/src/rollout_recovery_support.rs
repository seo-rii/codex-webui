use super::*;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RolloutRecoveryInfoPayload {
    pub(crate) available: bool,
    pub(crate) issue: Option<String>,
    pub(crate) total_lines: usize,
    pub(crate) recoverable_lines: usize,
    pub(crate) skipped_lines: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RolloutRecoveryPlanPayload {
    pub(crate) info: RolloutRecoveryInfoPayload,
    pub(crate) recovered_content: String,
}

fn normalize_rollout_line(raw_line: &str) -> Option<String> {
    let trimmed = raw_line
        .trim_start_matches('\u{feff}')
        .replace('\0', "")
        .trim()
        .to_string();
    if trimmed.is_empty() {
        return None;
    }

    let mut candidates = vec![trimmed.clone()];
    if let (Some(first_brace), Some(last_brace)) = (trimmed.find('{'), trimmed.rfind('}')) {
        let sliced = trimmed[first_brace..=last_brace].trim().to_string();
        if !sliced.is_empty() && sliced != trimmed {
            candidates.push(sliced);
        }
    }

    for candidate in candidates {
        if let Ok(parsed) = serde_json::from_str::<Value>(&candidate) {
            if let Ok(normalized) = serde_json::to_string(&parsed) {
                return Some(normalized);
            }
        }
    }

    None
}

pub(crate) fn inspect_rollout_recovery_content(buffer: &[u8]) -> RolloutRecoveryPlanPayload {
    let mut issue = std::str::from_utf8(buffer)
        .err()
        .map(|_| "invalidUtf8".to_string());
    let decoded = String::from_utf8_lossy(buffer);
    let mut total_lines = 0_usize;
    let mut recoverable_lines = 0_usize;
    let mut skipped_lines = 0_usize;
    let mut recovered_lines = Vec::new();

    for raw_line in decoded.lines() {
        if raw_line.trim().is_empty() {
            continue;
        }

        total_lines += 1;
        let Some(normalized) = normalize_rollout_line(raw_line) else {
            skipped_lines += 1;
            continue;
        };

        recoverable_lines += 1;
        recovered_lines.push(normalized);
    }

    if issue.is_none() && skipped_lines > 0 {
        issue = Some("invalidJson".to_string());
    }

    RolloutRecoveryPlanPayload {
        info: RolloutRecoveryInfoPayload {
            available: recoverable_lines > 0
                && (issue.as_deref() == Some("invalidUtf8") || skipped_lines > 0),
            issue,
            total_lines,
            recoverable_lines,
            skipped_lines,
        },
        recovered_content: if recovered_lines.is_empty() {
            String::new()
        } else {
            format!("{}\n", recovered_lines.join("\n"))
        },
    }
}

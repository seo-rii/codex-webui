export const ATTACHMENT_PREAMBLE_START = "[[codex-webui-attachments]]";
export const ATTACHMENT_PREAMBLE_END = "[[/codex-webui-attachments]]";

export function buildAttachmentPreamble(paths: string[]) {
  if (paths.length === 0) {
    return "";
  }
  return `${ATTACHMENT_PREAMBLE_START}\n${paths.join("\n")}\n${ATTACHMENT_PREAMBLE_END}`;
}

export function stripAttachmentPreamble(text: string) {
  const pattern = new RegExp(
    `^${escapeRegExp(ATTACHMENT_PREAMBLE_START)}\\n[\\s\\S]*?\\n${escapeRegExp(ATTACHMENT_PREAMBLE_END)}\\n\\n?`,
    "u"
  );
  return text.replace(pattern, "");
}

export function extractAttachmentPaths(text: string) {
  const pattern = new RegExp(
    `^${escapeRegExp(ATTACHMENT_PREAMBLE_START)}\\n([\\s\\S]*?)\\n${escapeRegExp(ATTACHMENT_PREAMBLE_END)}\\n?`,
    "u"
  );
  const match = text.match(pattern);
  if (!match?.[1]) {
    return [];
  }
  return match[1]
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);
}

function escapeRegExp(value: string) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

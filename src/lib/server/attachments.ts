import { randomUUID } from "node:crypto";
import fsp from "node:fs/promises";
import path from "node:path";

import { error } from "@sveltejs/kit";

import { buildAttachmentPreamble } from "$lib/attachments";
import type { AttachmentRecord } from "$lib/types";

import { getRuntimeConfig } from "./env";
import { ensureDataDirectories, getThreadUploadsDir, sanitizeFileName } from "./fs";

const IMAGE_MIME_TYPES = new Set(["image/png", "image/jpeg", "image/webp", "image/gif"]);

type StoredAttachment = AttachmentRecord & {
  filePath: string;
  metaPath: string;
};

function getAttachmentPaths(threadId: string, attachmentId: string, originalName: string) {
  const threadDir = getThreadUploadsDir(threadId);
  const base = `${attachmentId}-${sanitizeFileName(originalName)}`;
  return {
    threadDir,
    filePath: path.join(threadDir, base),
    metaPath: path.join(threadDir, `${base}.json`)
  };
}

export async function saveUploads(threadId: string, uploads: File[]) {
  await ensureDataDirectories();
  const threadDir = getThreadUploadsDir(threadId);
  await fsp.mkdir(threadDir, { recursive: true });

  const maxUploadBytes = getRuntimeConfig().maxUploadBytes;
  const results: AttachmentRecord[] = [];

  for (const upload of uploads) {
    if (upload.size > maxUploadBytes) {
      throw error(413, `Upload exceeds the ${Math.round(maxUploadBytes / (1024 * 1024))}MB limit.`);
    }

    const attachmentId = randomUUID();
    const { filePath, metaPath } = getAttachmentPaths(threadId, attachmentId, upload.name);
    const mimeType = upload.type || "application/octet-stream";
    const attachment: StoredAttachment = {
      id: attachmentId,
      originalName: upload.name,
      path: filePath,
      mimeType,
      size: upload.size,
      kind: IMAGE_MIME_TYPES.has(mimeType) ? "image" : "file",
      createdAt: new Date().toISOString(),
      filePath,
      metaPath
    };

    await fsp.writeFile(filePath, Buffer.from(await upload.arrayBuffer()));
    await fsp.writeFile(metaPath, JSON.stringify(attachment, null, 2), "utf8");

    results.push({
      id: attachment.id,
      originalName: attachment.originalName,
      path: attachment.path,
      mimeType: attachment.mimeType,
      size: attachment.size,
      kind: attachment.kind,
      createdAt: attachment.createdAt
    });
  }

  return results;
}

async function readStoredAttachment(metaPath: string): Promise<StoredAttachment> {
  const raw = await fsp.readFile(metaPath, "utf8");
  const parsed = JSON.parse(raw) as StoredAttachment;
  return parsed;
}

export async function listAttachments(threadId: string): Promise<AttachmentRecord[]> {
  const threadDir = getThreadUploadsDir(threadId);
  try {
    const entries = await fsp.readdir(threadDir);
    const attachments = await Promise.all(
      entries
        .filter((entry: string) => entry.endsWith(".json"))
        .map((entry: string) => readStoredAttachment(path.join(threadDir, entry)))
    );

    return attachments
      .sort((left: StoredAttachment, right: StoredAttachment) => right.createdAt.localeCompare(left.createdAt))
      .map(({ filePath: _filePath, metaPath: _metaPath, ...attachment }: StoredAttachment) => attachment);
  } catch {
    return [];
  }
}

export async function removeAttachment(threadId: string, attachmentId: string) {
  const attachments = await listAttachments(threadId);
  const target = attachments.find((attachment: AttachmentRecord) => attachment.id === attachmentId);
  if (!target) {
    throw error(404, "Attachment not found.");
  }

  const { filePath, metaPath } = getAttachmentPaths(threadId, attachmentId, target.originalName);
  await Promise.allSettled([fsp.rm(filePath, { force: true }), fsp.rm(metaPath, { force: true })]);
}

export function buildTurnInput(prompt: string, attachments: AttachmentRecord[]) {
  const textAttachments = attachments.filter((attachment) => attachment.kind === "file");
  const imageAttachments = attachments.filter((attachment) => attachment.kind === "image");

  const note = buildAttachmentPreamble(textAttachments.map((attachment) => attachment.path));
  const message = note ? `${note}\n\n${prompt}` : prompt;

  return [
    { type: "text", text: message, text_elements: [] },
    ...imageAttachments.map((attachment) => ({ type: "localImage", path: attachment.path }))
  ];
}

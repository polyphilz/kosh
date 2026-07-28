import { invoke } from "@tauri-apps/api/core";

const UPLOAD_METADATA_LIMIT = 8 * 1024;
const UUID_V7 = /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

export interface MediaLimits {
  maxAttachmentBytes: number;
  maxAttachmentsPerDraft: number;
  maxProtocolResponseBytes: number;
  draftLeaseDurationMs: number;
  orphanGracePeriodMs: number;
  maxReapsPerMaintenance: number;
}

export type AttachmentKind = "IMAGE" | "PDF" | "TEXT" | "BINARY";

export interface AttachmentRecord {
  id: string;
  ingestLeaseId: string;
  displayFilename: string;
  mediaType: string;
  byteLength: number;
  kind: AttachmentKind;
}

export interface AttachmentUpload {
  draftId: string;
  displayFilename: string;
  mediaType: string;
}

export interface MediaIntegrityReport {
  missingBlobAttachmentIds: string[];
  corruptBlobSha256: string[];
  extraBlobSha256: string[];
  orphanedAttachmentIds: string[];
}

export interface MediaMaintenanceReport {
  inspectedAtMs: number;
  integrity: MediaIntegrityReport;
  cleanup: {
    retiredAttachmentCount: number;
    deletedBlobCount: number;
    reclaimedBytes: number;
  };
}

export function loadMediaLimits(): Promise<MediaLimits> {
  return invoke<MediaLimits>("media_limits");
}

export async function ingestAttachmentBytes(
  input: AttachmentUpload,
  bytes: ArrayBuffer | Uint8Array,
): Promise<AttachmentRecord> {
  const limits = await loadMediaLimits();
  return ingestBoundedAttachmentBytes(input, bytes, limits);
}

async function ingestBoundedAttachmentBytes(
  input: AttachmentUpload,
  bytes: ArrayBuffer | Uint8Array,
  limits: MediaLimits,
): Promise<AttachmentRecord> {
  const source = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  validateAttachmentSize(source.byteLength, limits);
  const metadata = new TextEncoder().encode(JSON.stringify(input));
  if (metadata.byteLength === 0 || metadata.byteLength > UPLOAD_METADATA_LIMIT) {
    throw new Error("Attachment metadata is too large.");
  }
  const payloadLength = 4 + metadata.byteLength + source.byteLength;
  if (!Number.isSafeInteger(payloadLength)) {
    throw new Error("Attachment upload size is not representable.");
  }
  const payload = new Uint8Array(payloadLength);
  new DataView(payload.buffer).setUint32(0, metadata.byteLength, false);
  payload.set(metadata, 4);
  payload.set(source, 4 + metadata.byteLength);
  return invoke<AttachmentRecord>("ingest_attachment", payload);
}

export async function ingestAttachmentFile(draftId: string, file: File): Promise<AttachmentRecord> {
  const limits = await loadMediaLimits();
  validateAttachmentSize(file.size, limits);
  return ingestBoundedAttachmentBytes(
    {
      draftId,
      displayFilename: file.name,
      mediaType: file.type || "application/octet-stream",
    },
    await file.arrayBuffer(),
    limits,
  );
}

export function attachmentMediaUrl(attachmentId: string): string {
  if (!UUID_V7.test(attachmentId)) {
    throw new Error("Attachment ID must be a canonical UUIDv7.");
  }
  return `kosh-media://localhost/attachment/${attachmentId}`;
}

export function scanMediaIntegrity(): Promise<MediaIntegrityReport> {
  return invoke<MediaIntegrityReport>("media_integrity_scan");
}

export function maintainMedia(): Promise<MediaMaintenanceReport> {
  return invoke<MediaMaintenanceReport>("maintain_media");
}

function validateAttachmentSize(byteLength: number, limits: MediaLimits): void {
  if (byteLength === 0) {
    throw new Error("The selected attachment is empty.");
  }
  if (!Number.isSafeInteger(byteLength) || byteLength > limits.maxAttachmentBytes) {
    throw new Error(`The selected attachment is larger than ${limits.maxAttachmentBytes} bytes.`);
  }
}

import { invoke } from "@tauri-apps/api/core";
import { TauriCommand } from "../tauriProtocol";
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

export interface MediaIntegrityReport {
  missingBlobAttachmentIds: string[];
  corruptBlobSha256: string[];
  extraBlobSha256: string[];
  orphanedAttachmentIds: string[];
  diagnosticsTruncated: boolean;
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
  return invoke<MediaLimits>(TauriCommand.MediaLimits);
}

export function attachmentMediaUrl(attachmentId: string): string {
  if (!UUID_V7.test(attachmentId)) {
    throw new Error("Attachment ID must be a canonical UUIDv7.");
  }
  return `kosh-media://localhost/attachment/${attachmentId}`;
}

export function scanMediaIntegrity(): Promise<MediaIntegrityReport> {
  return invoke<MediaIntegrityReport>(TauriCommand.MediaIntegrityScan);
}

export function maintainMedia(): Promise<MediaMaintenanceReport> {
  return invoke<MediaMaintenanceReport>(TauriCommand.MaintainMedia);
}

export interface RuntimeProbe {
  dataDir: string;
  nowMs: number;
  requestId: string;
}

export type SemanticRuntimePhase =
  | "NOT_DOWNLOADED"
  | "VERIFICATION_REQUIRED"
  | "DOWNLOADING"
  | "VERIFYING"
  | "STARTING"
  | "READY"
  | "UNAVAILABLE"
  | "FAILED";

export interface SemanticRuntimeStatus {
  phase: SemanticRuntimePhase;
  downloadedBytes: number;
  modelBytes: number;
  modelDiskUsageBytes: number;
  runtimeRunning: boolean;
  verified: boolean;
  message: string | null;
}

export interface SemanticRuntimeLogs {
  text: string;
  truncated: boolean;
}

export type PassageEmbeddingIndexPhase = "WAITING_FOR_RUNTIME" | "INDEXING" | "READY" | "FAILED";

export interface PassageEmbeddingIndexStatus {
  phase: PassageEmbeddingIndexPhase;
  embeddingIndexId: string;
  indexKey: string;
  indexedPassages: number;
  totalPassages: number;
  active: boolean;
  message: string | null;
}

export interface SourceDraft {
  label: string | null;
  url: string | null;
}

export interface TidbitDraft {
  title: string | null;
  bodyMarkdown: string;
  sources: SourceDraft[];
}

export interface EditTidbitInput extends TidbitDraft {
  id: string;
  expectedRevisionId: string;
}

export interface DeleteTidbitInput {
  id: string;
  expectedRevisionId: string;
}

export interface RestoreTidbitInput {
  id: string;
  expectedRevisionId: string;
}

export type CitationState = "CURRENT" | "HISTORICAL";

export type CitationLocator =
  | {
      kind: "MARKDOWN_BLOCKS";
      startBlock: number;
      endBlock: number;
      sourceStartByte: number | null;
      sourceEndByte: number | null;
      startChar: number | null;
      endChar: number | null;
      startLine: number | null;
      endLine: number | null;
    }
  | {
      kind: "PDF_PAGE";
      page: number;
    }
  | {
      kind: "OCR_REGION";
      page: number | null;
      region: unknown;
    }
  | {
      kind: "TEXT_LINES";
      startLine: number;
      endLine: number;
    };

export interface CitationTidbit {
  id: string;
  revisionId: string;
  revisionNumber: number;
  title: string | null;
  displayTitle: string;
  deleted: boolean;
}

export interface CitationAttachment {
  id: string;
  extractionId: string;
  displayFilename: string;
  mediaType: string;
  deleted: boolean;
}

export interface CitationResolution {
  passageId: string;
  excerpt: string;
  headingContext: string[];
  constructionVersion: string;
  state: CitationState;
  locator: CitationLocator;
  tidbit: CitationTidbit | null;
  attachment: CitationAttachment | null;
  sources: TidbitSource[];
}

export type LexicalSearchMode = "DEFAULT" | "EXACT";

export type SearchField =
  | "TITLE"
  | "HEADING_CONTEXT"
  | "BODY"
  | "SOURCE_LABEL"
  | "SOURCE_DOMAIN"
  | "ATTACHMENT_NAME"
  | "EXTRACTED_TEXT";

export interface SearchPassagesInput {
  query: string;
  mode: LexicalSearchMode;
  limit: number;
}

export interface SearchHighlight {
  field: SearchField;
  startChar: number;
  endChar: number;
}

export interface PassageSearchResult {
  passageId: string;
  score: number;
  matchedFields: SearchField[];
  highlights: SearchHighlight[];
  citation: CitationResolution;
}

export type SearchExecutionMode = "EXACT" | "HYBRID" | "LEXICAL_ONLY";

export type SemanticSearchReadiness =
  | "READY"
  | "INDEXING"
  | "WAITING_FOR_RUNTIME"
  | "FAILED"
  | "NOT_REQUESTED";

export interface SearchPassagesResponse {
  results: PassageSearchResult[];
  executionMode: SearchExecutionMode;
  semanticReadiness: SemanticSearchReadiness;
}

export interface SaveDraftInput extends TidbitDraft {
  contextKey: string;
  tidbitId: string | null;
  baseRevisionId: string | null;
}

export interface ClearDraftInput {
  contextKey: string;
  expectedUpdatedAtMs: number;
}

export interface DraftRecord extends SaveDraftInput {
  id: string;
  createdAtMs: number;
  updatedAtMs: number;
}

export type ImageOcrStatus = "PENDING" | "RUNNING" | "RETRY_WAIT" | "READY" | "FAILED";

export interface ImageRecord {
  id: string;
  ingestLeaseId: string;
  displayFilename: string;
  mediaType: string;
  byteLength: number;
  kind: "IMAGE";
  naturalWidth: number;
  naturalHeight: number;
  ocrStatus: ImageOcrStatus;
  ocrError: string | null;
}

export interface ImageStatusRecord {
  attachmentId: string;
  naturalWidth: number;
  naturalHeight: number;
  ocrStatus: ImageOcrStatus;
  ocrError: string | null;
}

export interface ImageDropIngestResult {
  images: ImageRecord[];
  failures: Array<{
    filename: string;
    message: string;
  }>;
}

export interface ImageOcrDiagnostics {
  pending: number;
  running: number;
  retryWait: number;
  ready: number;
  failed: number;
  oldestEligibleAtMs: number | null;
  lastError: string | null;
}

export interface TidbitSource {
  id: string;
  label: string | null;
  url: string | null;
}

export interface TidbitRecord {
  id: string;
  currentRevisionId: string;
  revisionNumber: number;
  createdAtMs: number;
  updatedAtMs: number;
  deletedAtMs: number | null;
  title: string | null;
  displayTitle: string;
  bodyMarkdown: string;
  sources: TidbitSource[];
}

export interface TidbitListCursor {
  updatedAtMs: number;
  id: string;
}

export interface ListTidbitsInput {
  limit: number;
  cursor: TidbitListCursor | null;
}

export interface TidbitListItem {
  id: string;
  currentRevisionId: string;
  createdAtMs: number;
  updatedAtMs: number;
  title: string | null;
  displayTitle: string;
  bodyPreview: string;
}

export interface TidbitListPage {
  items: TidbitListItem[];
  nextCursor: TidbitListCursor | null;
}

export interface Backend {
  runtimeProbe(): Promise<RuntimeProbe>;
  semanticRuntimeStatus(): Promise<SemanticRuntimeStatus>;
  prepareSemanticRuntime(): Promise<SemanticRuntimeStatus>;
  retrySemanticRuntime(): Promise<SemanticRuntimeStatus>;
  repairSemanticRuntime(): Promise<SemanticRuntimeStatus>;
  semanticRuntimeLogs(): Promise<SemanticRuntimeLogs>;
  passageEmbeddingIndexStatus(): Promise<PassageEmbeddingIndexStatus>;
  createTidbit(input: TidbitDraft): Promise<TidbitRecord>;
  loadTidbit(id: string): Promise<TidbitRecord>;
  listTidbits(input: ListTidbitsInput): Promise<TidbitListPage>;
  editTidbit(input: EditTidbitInput): Promise<TidbitRecord>;
  deleteTidbit(input: DeleteTidbitInput): Promise<TidbitRecord>;
  restoreTidbit(input: RestoreTidbitInput): Promise<TidbitRecord>;
  resolveCitation(passageId: string): Promise<CitationResolution>;
  searchPassages(input: SearchPassagesInput): Promise<SearchPassagesResponse>;
  saveDraft(input: SaveDraftInput): Promise<DraftRecord>;
  loadDraft(contextKey: string): Promise<DraftRecord | null>;
  clearDraft(input: ClearDraftInput): Promise<boolean>;
  pickImage(draftId: string): Promise<ImageRecord | null>;
  captureClipboardImage(): Promise<string>;
  ingestClipboardImage(captureId: string, draftId: string): Promise<ImageRecord>;
  ingestDroppedImages(dropId: string, draftId: string): Promise<ImageDropIngestResult>;
  imageStatus(attachmentId: string): Promise<ImageStatusRecord>;
  retryImageOcr(attachmentId: string): Promise<ImageStatusRecord>;
  imageOcrDiagnostics(): Promise<ImageOcrDiagnostics>;
}

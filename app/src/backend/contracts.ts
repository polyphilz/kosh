export interface RuntimeProbe {
  dataDir: string;
  nowMs: number;
  requestId: string;
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
      sourceStartByte: number;
      sourceEndByte: number;
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
  createTidbit(input: TidbitDraft): Promise<TidbitRecord>;
  loadTidbit(id: string): Promise<TidbitRecord>;
  listTidbits(input: ListTidbitsInput): Promise<TidbitListPage>;
  editTidbit(input: EditTidbitInput): Promise<TidbitRecord>;
  deleteTidbit(input: DeleteTidbitInput): Promise<TidbitRecord>;
  restoreTidbit(input: RestoreTidbitInput): Promise<TidbitRecord>;
  resolveCitation(passageId: string): Promise<CitationResolution>;
  saveDraft(input: SaveDraftInput): Promise<DraftRecord>;
  loadDraft(contextKey: string): Promise<DraftRecord | null>;
  clearDraft(input: ClearDraftInput): Promise<boolean>;
}

export interface Tidbit {
  id: string;
  currentRevisionId: string;
  createdAtMs: number;
  updatedAtMs: number;
  deletedAtMs: number | null;
}

export interface TidbitRevision {
  id: string;
  tidbitId: string;
  title: string | null;
  bodyMarkdown: string;
  createdAtMs: number;
}

export interface Source {
  id: string;
  label: string;
  url: string | null;
}

export interface Attachment {
  id: string;
  filename: string;
  mediaType: string;
  byteLength: number;
  extractionState: "pending" | "ready" | "opaque" | "failed";
}

export interface Passage {
  id: string;
  revisionId: string;
  content: string;
  locator: {
    kind: "markdown-blocks";
    startBlock: number;
    endBlock: number;
  };
}

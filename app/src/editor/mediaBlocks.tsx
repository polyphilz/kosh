import { createReactBlockSpec, type ReactCustomBlockRenderProps } from "@blocknote/react";
import {
  createContext,
  useContext,
  useEffect,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from "react";
import type {
  GenericAttachmentStatusRecord,
  ImageStatusRecord,
  PdfStatusRecord,
  SelectedAttachmentRecord,
} from "../backend/contracts";
import { attachmentMediaUrl } from "../media/gateway";
import { useKoshEditorDisabled } from "./interactionState";
import { clampImageWidth, initialImageWidth } from "./mediaSizing";

const ACTIVE_IMAGE_STATUSES = new Set(["PENDING", "RUNNING", "RETRY_WAIT"]);
const ACTIVE_PDF_STATUSES = new Set(["PENDING", "RUNNING", "RETRY_WAIT"]);
const STATUS_POLL_MS = 1_500;
const STATUS_POLL_MAX_MS = 5 * 60_000;

export interface KoshMediaActions {
  attachmentStatus?: (attachmentId: string) => Promise<GenericAttachmentStatusRecord>;
  imageStatus?: (attachmentId: string) => Promise<ImageStatusRecord>;
  mediaUrl?: (attachmentId: string) => string;
  onError?: (error: unknown) => void;
  openAttachmentExternal?: (attachmentId: string) => Promise<void>;
  openPdfExternal?: (attachmentId: string) => Promise<void>;
  pdfStatus?: (attachmentId: string) => Promise<PdfStatusRecord>;
  revealAttachmentInFinder?: (attachmentId: string) => Promise<void>;
  retryImageOcr?: (attachmentId: string) => Promise<ImageStatusRecord>;
  retryPdfExtraction?: (attachmentId: string) => Promise<PdfStatusRecord>;
}

const KoshMediaActionsContext = createContext<KoshMediaActions>({});

export function KoshMediaActionsProvider({
  actions,
  children,
}: {
  actions: KoshMediaActions;
  children: ReactNode;
}) {
  return (
    <KoshMediaActionsContext.Provider value={actions}>{children}</KoshMediaActionsContext.Provider>
  );
}

const imageConfig = {
  type: "koshImage",
  propSchema: {
    attachmentId: { default: "" },
    altText: { default: "" },
    caption: { default: "" },
    naturalHeight: { default: 0 },
    naturalWidth: { default: 0 },
    ocrError: { default: "" },
    ocrStatus: {
      default: "PENDING",
      values: ["PENDING", "RUNNING", "RETRY_WAIT", "READY", "FAILED"] as const,
    },
    widthPercent: { default: 100 },
  },
  content: "none",
} as const;

const image = createReactBlockSpec(imageConfig, {
  meta: { isolating: true, selectable: true },
  render: ({ block, editor }) => <KoshImageBlock block={block} editor={editor} />,
});

const pendingMedia = createReactBlockSpec(
  {
    type: "koshPendingMedia",
    propSchema: {
      label: { default: "Adding attachment" },
      requestId: { default: "" },
    },
    content: "none",
  },
  {
    meta: { isolating: true, selectable: true },
    render: ({ block }) => (
      <div
        aria-label={block.props.label}
        className="kosh-blocknote-pending-media"
        contentEditable={false}
        data-request-id={block.props.requestId}
        role="status"
      >
        {block.props.label}…
      </div>
    ),
  },
);

const pdfConfig = {
  type: "koshPdf",
  propSchema: {
    attachmentId: { default: "" },
    displayFilename: { default: "PDF attachment" },
    extractedPageCount: { default: 0 },
    extractionError: { default: "" },
    extractionStatus: {
      default: "PENDING",
      values: ["PENDING", "RUNNING", "RETRY_WAIT", "READY", "FAILED"] as const,
    },
    nextAttemptAtMs: { default: 0 },
    pageCount: { default: 0 },
    unavailablePageCount: { default: 0 },
  },
  content: "none",
} as const;

const pdf = createReactBlockSpec(pdfConfig, {
  meta: { isolating: true, selectable: true },
  render: ({ block, editor }) => <KoshPdfBlock block={block} editor={editor} />,
});

const fileAttachmentConfig = {
  type: "koshFileAttachment",
  propSchema: {
    attachmentId: { default: "" },
    byteLength: { default: 0 },
    caption: { default: "" },
    displayFilename: { default: "Attachment" },
    extractedLineCount: { default: 0 },
    extractionError: { default: "" },
    extractionStatus: {
      default: "NOT_APPLICABLE",
      values: ["READY", "FAILED", "NOT_APPLICABLE"] as const,
    },
    kind: { default: "BINARY", values: ["TEXT", "BINARY"] as const },
    mediaType: { default: "application/octet-stream" },
  },
  content: "none",
} as const;

const fileAttachment = createReactBlockSpec(fileAttachmentConfig, {
  meta: { isolating: true, selectable: true },
  render: ({ block, editor }) => <KoshFileBlock block={block} editor={editor} />,
});

export const koshMediaBlockSpecs = {
  koshImage: image(),
  koshPendingMedia: pendingMedia(),
  koshPdf: pdf(),
  koshFileAttachment: fileAttachment(),
};

interface MediaPartialBlock {
  props: Record<string, boolean | number | string>;
  type: "koshFileAttachment" | "koshImage" | "koshPdf";
}

export function selectedAttachmentToMediaBlock(
  selection: SelectedAttachmentRecord,
  editorWidth = 0,
): MediaPartialBlock {
  switch (selection.recordKind) {
    case "IMAGE": {
      const record = selection.record;
      return {
        type: "koshImage",
        props: {
          attachmentId: record.id,
          altText: "",
          caption: "",
          naturalHeight: record.naturalHeight,
          naturalWidth: record.naturalWidth,
          ocrError: record.ocrError ?? "",
          ocrStatus: record.ocrStatus,
          widthPercent: initialImageWidth(record.naturalWidth, editorWidth),
        },
      };
    }
    case "PDF": {
      const record = selection.record;
      return {
        type: "koshPdf",
        props: {
          attachmentId: record.id,
          displayFilename: record.displayFilename,
          extractedPageCount: 0,
          extractionError: record.extractionError ?? "",
          extractionStatus: record.extractionStatus,
          nextAttemptAtMs: 0,
          pageCount: record.pageCount,
          unavailablePageCount: 0,
        },
      };
    }
    case "GENERIC": {
      const record = selection.record;
      return {
        type: "koshFileAttachment",
        props: {
          attachmentId: record.id,
          byteLength: record.byteLength,
          caption: "",
          displayFilename: record.displayFilename,
          extractedLineCount: record.extractedLineCount,
          extractionError: record.extractionError ?? "",
          extractionStatus: record.extractionStatus,
          kind: record.kind,
          mediaType: record.mediaType,
        },
      };
    }
  }
}

type ImageRenderProps = ReactCustomBlockRenderProps<typeof imageConfig>;
type PdfRenderProps = ReactCustomBlockRenderProps<typeof pdfConfig>;
type FileRenderProps = ReactCustomBlockRenderProps<typeof fileAttachmentConfig>;

function KoshImageBlock({ block, editor }: ImageRenderProps) {
  const actions = useContext(KoshMediaActionsContext);
  const locked = useKoshEditorDisabled() || !editor.isEditable;
  const [pollRevision, setPollRevision] = useState(0);
  const [status, setStatus] = useState<ImageStatusRecord | null>(null);
  const figureRef = useRef<HTMLElement>(null);
  const attachmentId = block.props.attachmentId;
  const renderedStatus = status?.ocrStatus ?? block.props.ocrStatus;
  const renderedError = status?.ocrError ?? block.props.ocrError;

  useEffect(() => {
    if (!actions.imageStatus || !attachmentId) return;
    let active = true;
    let timer: number | undefined;
    const load = () => {
      void actions.imageStatus!(attachmentId)
        .then((record) => {
          if (!active || record.attachmentId !== attachmentId) return;
          setStatus(record);
          if (ACTIVE_IMAGE_STATUSES.has(record.ocrStatus)) {
            timer = window.setTimeout(load, statusPollDelay(record.nextAttemptAtMs));
          }
        })
        .catch((error: unknown) => actions.onError?.(error));
    };
    load();
    return () => {
      active = false;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [actions, attachmentId, pollRevision]);

  const updateWidth = (widthPercent: number) => {
    if (locked) return;
    editor.updateBlock(block, { props: { widthPercent: clampImageWidth(widthPercent) } });
  };

  const beginResize = (event: ReactPointerEvent<HTMLButtonElement>) => {
    if (locked || event.button !== 0) return;
    event.preventDefault();
    event.stopPropagation();
    const parentWidth = figureRef.current?.parentElement?.getBoundingClientRect().width ?? 0;
    if (parentWidth <= 0) return;
    const handle = event.currentTarget;
    const pointerId = event.pointerId;
    const startX = event.clientX;
    const startWidth = block.props.widthPercent;
    let nextWidth = startWidth;
    handle.setPointerCapture?.(pointerId);
    const move = (moveEvent: PointerEvent) => {
      nextWidth = clampImageWidth(startWidth + ((moveEvent.clientX - startX) / parentWidth) * 100);
      if (figureRef.current) figureRef.current.style.width = `${nextWidth}%`;
    };
    const finish = () => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", finish);
      handle.removeEventListener("pointercancel", cancel);
      if (handle.hasPointerCapture?.(pointerId)) handle.releasePointerCapture(pointerId);
      updateWidth(nextWidth);
    };
    const cancel = () => {
      nextWidth = startWidth;
      finish();
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", finish);
    handle.addEventListener("pointercancel", cancel);
  };

  return (
    <figure
      aria-label={block.props.altText ? `Image: ${block.props.altText}` : "Note image"}
      className="kosh-blocknote-image"
      contentEditable={false}
      data-kosh-image="true"
      onKeyDown={(event) => {
        if (!editor.isEditable || !event.altKey || !["ArrowLeft", "ArrowRight"].includes(event.key))
          return;
        event.preventDefault();
        updateWidth(block.props.widthPercent + (event.key === "ArrowLeft" ? -5 : 5));
      }}
      ref={figureRef}
      style={{ width: `${clampImageWidth(block.props.widthPercent)}%` }}
      tabIndex={0}
    >
      <img alt={block.props.altText} draggable={false} src={safeMediaUrl(actions, attachmentId)} />
      <button
        aria-label="Resize image"
        className="kosh-blocknote-image__resize"
        disabled={locked}
        onPointerDown={beginResize}
        type="button"
      />
      <div className="kosh-blocknote-image__fields">
        <label>
          <span>Alt text</span>
          <input
            disabled={locked}
            maxLength={500}
            onChange={(event) =>
              editor.updateBlock(block, { props: { altText: event.currentTarget.value } })
            }
            placeholder="Describe this image"
            value={block.props.altText}
          />
        </label>
        <label>
          <span>Caption</span>
          <input
            disabled={locked}
            maxLength={2_000}
            onChange={(event) =>
              editor.updateBlock(block, { props: { caption: event.currentTarget.value } })
            }
            placeholder="Optional caption"
            value={block.props.caption}
          />
        </label>
      </div>
      <div className="kosh-blocknote-media__footer">
        <span aria-live="polite" title={renderedError}>
          {imageStatusText(renderedStatus, renderedError)}
        </span>
        {renderedStatus === "FAILED" && actions.retryImageOcr && (
          <button
            disabled={locked}
            onClick={() =>
              void actions.retryImageOcr!(attachmentId)
                .then((record) => {
                  setStatus(record);
                  if (ACTIVE_IMAGE_STATUSES.has(record.ocrStatus)) {
                    setPollRevision((revision) => revision + 1);
                  }
                })
                .catch((error: unknown) => actions.onError?.(error))
            }
            type="button"
          >
            Retry text recognition
          </button>
        )}
        <button disabled={locked} onClick={() => editor.removeBlocks([block])} type="button">
          Remove
        </button>
      </div>
    </figure>
  );
}

function KoshPdfBlock({ block, editor }: PdfRenderProps) {
  const actions = useContext(KoshMediaActionsContext);
  const disabled = useKoshEditorDisabled();
  const [pollRevision, setPollRevision] = useState(0);
  const [status, setStatus] = useState<PdfStatusRecord | null>(null);
  const attachmentId = block.props.attachmentId;

  useEffect(() => {
    if (!actions.pdfStatus || !attachmentId) return;
    let active = true;
    let timer: number | undefined;
    const load = () => {
      void actions.pdfStatus!(attachmentId)
        .then((record) => {
          if (!active || record.attachmentId !== attachmentId) return;
          setStatus(record);
          if (ACTIVE_PDF_STATUSES.has(record.extractionStatus)) {
            timer = window.setTimeout(load, statusPollDelay(record.nextAttemptAtMs));
          }
        })
        .catch((error: unknown) => actions.onError?.(error));
    };
    load();
    return () => {
      active = false;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [actions, attachmentId, pollRevision]);

  useEffect(() => {
    if (
      disabled ||
      !status?.displayFilename ||
      status.displayFilename === block.props.displayFilename
    ) {
      return;
    }
    const frame = window.requestAnimationFrame(() => {
      if (!editor.isEditable) return;
      editor.transact((transaction) => {
        editor.updateBlock(block, { props: { displayFilename: status.displayFilename } });
        transaction.setMeta("addToHistory", false);
      });
    });
    return () => window.cancelAnimationFrame(frame);
  }, [block, disabled, editor, status?.displayFilename]);

  const extractionStatus = status?.extractionStatus ?? block.props.extractionStatus;
  const extractionError = status?.extractionError ?? block.props.extractionError;
  return (
    <section className="kosh-blocknote-file" contentEditable={false} data-kosh-pdf="true">
      <span aria-hidden className="kosh-blocknote-file__icon">
        PDF
      </span>
      <div className="kosh-blocknote-file__details">
        <strong>{status?.displayFilename ?? block.props.displayFilename}</strong>
        <span title={extractionError}>
          {pdfStatusText(
            extractionStatus,
            extractionError,
            status?.pageCount ?? block.props.pageCount,
            status?.extractedPageCount ?? block.props.extractedPageCount,
            status?.unavailablePageCount ?? block.props.unavailablePageCount,
          )}
        </span>
      </div>
      <MediaButtons
        editor={editor}
        onOpen={actions.openPdfExternal ? () => actions.openPdfExternal!(attachmentId) : undefined}
        onRemove={() => editor.removeBlocks([block])}
        onRetry={
          extractionStatus === "FAILED" && actions.retryPdfExtraction
            ? () =>
                actions.retryPdfExtraction!(attachmentId).then((record) => {
                  setStatus(record);
                  if (ACTIVE_PDF_STATUSES.has(record.extractionStatus)) {
                    setPollRevision((revision) => revision + 1);
                  }
                })
            : undefined
        }
      />
    </section>
  );
}

function KoshFileBlock({ block, editor }: FileRenderProps) {
  const actions = useContext(KoshMediaActionsContext);
  const disabled = useKoshEditorDisabled();
  const locked = disabled || !editor.isEditable;
  const [status, setStatus] = useState<GenericAttachmentStatusRecord | null>(null);
  const attachmentId = block.props.attachmentId;
  useEffect(() => {
    if (!actions.attachmentStatus || !attachmentId) return;
    let active = true;
    void actions
      .attachmentStatus(attachmentId)
      .then((record) => {
        if (!active || record.attachmentId !== attachmentId) return;
        setStatus(record);
      })
      .catch((error: unknown) => actions.onError?.(error));
    return () => {
      active = false;
    };
  }, [actions, attachmentId]);

  useEffect(() => {
    if (
      disabled ||
      !status?.displayFilename ||
      status.displayFilename === block.props.displayFilename
    ) {
      return;
    }
    const frame = window.requestAnimationFrame(() => {
      if (!editor.isEditable) return;
      editor.transact((transaction) => {
        editor.updateBlock(block, { props: { displayFilename: status.displayFilename } });
        transaction.setMeta("addToHistory", false);
      });
    });
    return () => window.cancelAnimationFrame(frame);
  }, [block, disabled, editor, status?.displayFilename]);

  const filename = status?.displayFilename ?? block.props.displayFilename;
  return (
    <section className="kosh-blocknote-file" contentEditable={false} data-kosh-file="true">
      <span aria-hidden className="kosh-blocknote-file__icon">
        {fileIcon(filename)}
      </span>
      <div className="kosh-blocknote-file__details">
        <strong>{filename}</strong>
        <span title={status?.extractionError ?? block.props.extractionError}>
          {fileStatusText({
            byteLength: status?.byteLength ?? block.props.byteLength,
            extractedLineCount: status?.extractedLineCount ?? block.props.extractedLineCount,
            extractionError: status?.extractionError ?? block.props.extractionError,
            extractionStatus: status?.extractionStatus ?? block.props.extractionStatus,
            mediaType: status?.mediaType ?? block.props.mediaType,
          })}
        </span>
      </div>
      <MediaButtons
        editor={editor}
        onOpen={
          actions.openAttachmentExternal
            ? () => actions.openAttachmentExternal!(attachmentId)
            : undefined
        }
        onRemove={() => editor.removeBlocks([block])}
        onReveal={
          actions.revealAttachmentInFinder
            ? () => actions.revealAttachmentInFinder!(attachmentId)
            : undefined
        }
      />
      <label className="kosh-blocknote-file__caption">
        <span>Caption</span>
        <input
          aria-label="Attachment caption"
          disabled={locked}
          maxLength={2_000}
          onChange={(event) =>
            editor.updateBlock(block, { props: { caption: event.currentTarget.value } })
          }
          placeholder="Add a caption"
          value={block.props.caption}
        />
      </label>
    </section>
  );
}

function MediaButtons({
  editor,
  onOpen,
  onRemove,
  onRetry,
  onReveal,
}: {
  editor: { readonly isEditable: boolean };
  onOpen?: () => Promise<void>;
  onRemove: () => void;
  onRetry?: () => Promise<unknown>;
  onReveal?: () => Promise<void>;
}) {
  const actions = useContext(KoshMediaActionsContext);
  const locked = useKoshEditorDisabled() || !editor.isEditable;
  const invoke = (action: (() => Promise<unknown>) | undefined) =>
    action ? void action().catch((error: unknown) => actions.onError?.(error)) : undefined;
  return (
    <div className="kosh-blocknote-file__controls">
      {onOpen && (
        <button onClick={() => invoke(onOpen)} type="button">
          Open
        </button>
      )}
      {onReveal && (
        <button onClick={() => invoke(onReveal)} type="button">
          Reveal
        </button>
      )}
      {onRetry && (
        <button disabled={locked} onClick={() => invoke(onRetry)} type="button">
          Retry extraction
        </button>
      )}
      <button disabled={locked} onClick={onRemove} type="button">
        Remove
      </button>
    </div>
  );
}

function safeMediaUrl(actions: KoshMediaActions, attachmentId: string): string {
  try {
    return (actions.mediaUrl ?? attachmentMediaUrl)(attachmentId);
  } catch (error) {
    actions.onError?.(error);
    return "";
  }
}

function statusPollDelay(nextAttemptAtMs: number | null): number {
  if (nextAttemptAtMs === null || nextAttemptAtMs <= 0) return STATUS_POLL_MS;
  return Math.max(STATUS_POLL_MS, Math.min(STATUS_POLL_MAX_MS, nextAttemptAtMs - Date.now()));
}

function imageStatusText(status: string, error: string): string {
  switch (status) {
    case "RUNNING":
      return "Recognizing text…";
    case "RETRY_WAIT":
      return "Text recognition will retry";
    case "READY":
      return "Image text indexed";
    case "FAILED":
      return error || "Text recognition failed";
    default:
      return "Text recognition queued";
  }
}

function pdfStatusText(
  status: string,
  error: string,
  pageCount: number,
  extracted: number,
  unavailable: number,
): string {
  switch (status) {
    case "READY":
      return `${pageCount} page${pageCount === 1 ? "" : "s"} · ${extracted} searchable${
        unavailable ? ` · ${unavailable} unavailable` : ""
      }`;
    case "RUNNING":
      return `Extracting ${pageCount || ""} pages…`;
    case "RETRY_WAIT":
      return "Extraction will retry";
    case "FAILED":
      return error || "Extraction failed";
    default:
      return `Queued for extraction${pageCount ? ` · ${pageCount} pages` : ""}`;
  }
}

function fileStatusText(properties: {
  byteLength: number;
  extractedLineCount: number;
  extractionError: string;
  extractionStatus: string;
  mediaType: string;
}): string {
  const size = formatBytes(properties.byteLength);
  if (properties.extractionStatus === "READY") {
    return `${size} · ${properties.mediaType} · ${properties.extractedLineCount} line${
      properties.extractedLineCount === 1 ? "" : "s"
    } searchable`;
  }
  if (properties.extractionStatus === "FAILED") {
    return `${size} · ${properties.extractionError || "Text extraction failed"}`;
  }
  return `${size} · ${properties.mediaType} · Content not searchable`;
}

function formatBytes(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return "Unknown size";
  if (value < 1_024) return `${value} B`;
  if (value < 1_024 * 1_024) return `${(value / 1_024).toFixed(value < 10_240 ? 1 : 0)} KB`;
  return `${(value / (1_024 * 1_024)).toFixed(1)} MB`;
}

function fileIcon(filename: string): string {
  const extension = filename.split(".").pop();
  return !extension || extension === filename || extension.length > 5
    ? "FILE"
    : extension.toUpperCase();
}

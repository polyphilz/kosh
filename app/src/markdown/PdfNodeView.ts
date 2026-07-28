import type { Node as ProseMirrorNode } from "prosemirror-model";
import type { NodeView, EditorView } from "prosemirror-view";
import type { PdfStatusRecord } from "../backend/contracts";
import { KOSH_EDITOR_EDITABLE_EVENT } from "./editorEvents";

export interface PdfNodeViewActions {
  loadStatus?: (attachmentId: string) => Promise<PdfStatusRecord>;
  openExternal?: (attachmentId: string) => Promise<void>;
  retryExtraction?: (attachmentId: string) => Promise<PdfStatusRecord>;
}

export function pdfNodeView(
  initialNode: ProseMirrorNode,
  view: EditorView,
  getPos: () => number | undefined,
  actions: PdfNodeViewActions,
): NodeView {
  let node = initialNode;
  let destroyed = false;
  let poll: number | null = null;
  const dom = document.createElement("section");
  dom.className = "kosh-pdf-node";
  dom.dataset.koshAttachment = "true";

  const icon = document.createElement("span");
  icon.className = "kosh-pdf-node__icon";
  icon.ariaHidden = "true";
  icon.textContent = "PDF";

  const details = document.createElement("div");
  details.className = "kosh-pdf-node__details";
  const filename = document.createElement("strong");
  const status = document.createElement("span");
  status.className = "kosh-pdf-node__status";
  details.append(filename, status);

  const controls = document.createElement("div");
  controls.className = "kosh-pdf-node__controls";
  const open = button("Open original", () => {
    void actions.openExternal?.(node.attrs.attachmentId).catch(reportError);
  });
  const retry = button("Retry extraction", () => {
    if (!view.editable || !actions.retryExtraction) return;
    retry.disabled = true;
    void actions
      .retryExtraction(node.attrs.attachmentId)
      .then((record) => {
        installStatus(record);
        scheduleStatus();
      })
      .catch(reportError)
      .finally(() => {
        retry.disabled = !view.editable;
      });
  });
  const remove = button("Remove", () => {
    const position = getPos();
    if (position === undefined || !view.editable) return;
    view.dispatch(view.state.tr.delete(position, position + node.nodeSize).scrollIntoView());
    view.focus();
  });
  controls.append(open, retry, remove);
  dom.append(icon, details, controls);

  const render = () => {
    filename.textContent = node.attrs.displayFilename;
    filename.title = node.attrs.displayFilename;
    status.textContent = statusText(node.attrs);
    status.title = node.attrs.extractionError ?? "";
    retry.hidden = node.attrs.extractionStatus !== "FAILED";
    retry.disabled = !view.editable || !actions.retryExtraction;
    remove.disabled = !view.editable;
    open.disabled = !actions.openExternal;
  };

  const installStatus = (record: PdfStatusRecord) => {
    if (destroyed) return;
    const position = getPos();
    if (position === undefined) return;
    const attrs = {
      ...node.attrs,
      displayFilename: record.displayFilename,
      extractedPageCount: record.extractedPageCount,
      extractionError: record.extractionError,
      extractionStatus: record.extractionStatus,
      nextAttemptAtMs: record.nextAttemptAtMs,
      pageCount: record.pageCount,
      unavailablePageCount: record.unavailablePageCount,
    };
    view.dispatch(view.state.tr.setNodeMarkup(position, undefined, attrs));
  };

  const scheduleStatus = () => {
    if (
      destroyed ||
      !actions.loadStatus ||
      !["PENDING", "RUNNING", "RETRY_WAIT"].includes(node.attrs.extractionStatus)
    ) {
      return;
    }
    if (poll !== null) window.clearTimeout(poll);
    const retryAt = Number(node.attrs.nextAttemptAtMs);
    const delay =
      node.attrs.extractionStatus === "RETRY_WAIT" && retryAt > 0
        ? Math.min(Math.max(retryAt - Date.now(), 1_500), 300_000)
        : 2_000;
    poll = window.setTimeout(() => {
      poll = null;
      void actions.loadStatus!(node.attrs.attachmentId).then(installStatus).catch(reportError);
    }, delay);
  };

  const onEditableChange = () => render();
  view.dom.addEventListener(KOSH_EDITOR_EDITABLE_EVENT, onEditableChange);
  render();
  if (actions.loadStatus) {
    void actions.loadStatus(node.attrs.attachmentId).then(installStatus).catch(reportError);
  }

  return {
    destroy() {
      destroyed = true;
      if (poll !== null) window.clearTimeout(poll);
      view.dom.removeEventListener(KOSH_EDITOR_EDITABLE_EVENT, onEditableChange);
    },
    dom,
    ignoreMutation: () => true,
    stopEvent: (event) => controls.contains(event.target as Node),
    update(nextNode) {
      if (nextNode.type.name !== "kosh_attachment") return false;
      node = nextNode;
      render();
      scheduleStatus();
      return true;
    },
  };
}

function button(label: string, action: () => void): HTMLButtonElement {
  const element = document.createElement("button");
  element.className = "kosh-pdf-node__button";
  element.type = "button";
  element.textContent = label;
  element.addEventListener("click", action);
  return element;
}

function statusText(attrs: Record<string, unknown>): string {
  const pageCount = Number(attrs.pageCount);
  const extracted = Number(attrs.extractedPageCount);
  const unavailable = Number(attrs.unavailablePageCount);
  switch (attrs.extractionStatus) {
    case "READY":
      return `${pageCount} page${pageCount === 1 ? "" : "s"} · ${extracted} searchable${
        unavailable ? ` · ${unavailable} unavailable` : ""
      }`;
    case "RUNNING":
      return `Extracting ${pageCount || ""} pages…`;
    case "RETRY_WAIT":
      return "Extraction will retry";
    case "FAILED":
      return String(attrs.extractionError || "Extraction failed");
    default:
      return `Queued for extraction${pageCount ? ` · ${pageCount} pages` : ""}`;
  }
}

function reportError(error: unknown) {
  console.error("PDF attachment action failed", error);
}

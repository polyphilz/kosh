import type { Node as ProseMirrorNode } from "prosemirror-model";
import type { EditorView, NodeView } from "prosemirror-view";
import type { GenericAttachmentStatusRecord } from "../backend/contracts";
import { KOSH_EDITOR_EDITABLE_EVENT } from "./editorEvents";

export interface AttachmentNodeViewActions {
  loadStatus?: (attachmentId: string) => Promise<GenericAttachmentStatusRecord>;
  openExternal?: (attachmentId: string) => Promise<void>;
  pickReplacement?: () => Promise<ProseMirrorNode | null>;
  revealInFinder?: (attachmentId: string) => Promise<void>;
}

export function attachmentNodeView(
  initialNode: ProseMirrorNode,
  view: EditorView,
  getPos: () => number | undefined,
  actions: AttachmentNodeViewActions,
): NodeView {
  let node = initialNode;
  let destroyed = false;
  let replacing = false;
  const dom = document.createElement("section");
  dom.className = "kosh-file-node";
  dom.dataset.koshFileAttachment = "true";

  const icon = document.createElement("span");
  icon.className = "kosh-file-node__icon";
  icon.ariaHidden = "true";

  const details = document.createElement("div");
  details.className = "kosh-file-node__details";
  const filename = document.createElement("strong");
  const status = document.createElement("span");
  status.className = "kosh-file-node__status";
  details.append(filename, status);

  const controls = document.createElement("div");
  controls.className = "kosh-file-node__controls";
  const open = button("Open", () => {
    void actions.openExternal?.(node.attrs.attachmentId).catch(reportError);
  });
  const reveal = button("Reveal", () => {
    void actions.revealInFinder?.(node.attrs.attachmentId).catch(reportError);
  });
  const replace = button("Replace", () => {
    if (replacing || !view.editable || !actions.pickReplacement) return;
    replacing = true;
    replace.disabled = true;
    void actions
      .pickReplacement()
      .then((replacement) => {
        if (!replacement || destroyed) return;
        const position = getPos();
        if (position === undefined || !view.editable) return;
        view.dispatch(
          view.state.tr
            .replaceWith(position, position + node.nodeSize, replacement)
            .scrollIntoView(),
        );
        view.focus();
      })
      .catch(reportError)
      .finally(() => {
        replacing = false;
        replace.disabled = !view.editable || !actions.pickReplacement;
      });
  });
  const remove = button("Remove", () => {
    const position = getPos();
    if (position === undefined || !view.editable) return;
    view.dispatch(view.state.tr.delete(position, position + node.nodeSize).scrollIntoView());
    view.focus();
  });
  controls.append(open, reveal, replace, remove);

  const captionLabel = document.createElement("label");
  captionLabel.className = "kosh-file-node__caption";
  const captionLabelText = document.createElement("span");
  captionLabelText.className = "visually-hidden";
  captionLabelText.textContent = "Attachment caption";
  const caption = document.createElement("input");
  caption.type = "text";
  caption.maxLength = 2_000;
  caption.placeholder = "Add a caption";
  caption.setAttribute("aria-label", "Attachment caption");
  caption.addEventListener("input", () => {
    const position = getPos();
    if (position === undefined || !view.editable) return;
    view.dispatch(
      view.state.tr.setNodeMarkup(position, undefined, {
        ...node.attrs,
        caption: caption.value,
      }),
    );
  });
  captionLabel.append(captionLabelText, caption);
  dom.append(icon, details, controls, captionLabel);

  const render = () => {
    filename.textContent = node.attrs.displayFilename;
    filename.title = node.attrs.displayFilename;
    icon.textContent = fileIcon(node.attrs.displayFilename);
    status.textContent = statusText(node.attrs);
    status.title = node.attrs.extractionError ?? "";
    if (caption.value !== node.attrs.caption) {
      caption.value = node.attrs.caption;
    }
    caption.disabled = !view.editable;
    replace.disabled = replacing || !view.editable || !actions.pickReplacement;
    remove.disabled = !view.editable;
    open.disabled = !actions.openExternal;
    reveal.disabled = !actions.revealInFinder;
  };

  const installStatus = (record: GenericAttachmentStatusRecord) => {
    if (destroyed || record.attachmentId !== node.attrs.attachmentId) return;
    const position = getPos();
    if (position === undefined) return;
    view.dispatch(
      view.state.tr.setNodeMarkup(position, undefined, {
        ...node.attrs,
        byteLength: record.byteLength,
        displayFilename: record.displayFilename,
        extractedLineCount: record.extractedLineCount,
        extractionError: record.extractionError,
        extractionStatus: record.extractionStatus,
        kind: record.kind,
        mediaType: record.mediaType,
      }),
    );
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
      view.dom.removeEventListener(KOSH_EDITOR_EDITABLE_EVENT, onEditableChange);
    },
    dom,
    ignoreMutation: () => true,
    stopEvent: (event) =>
      controls.contains(event.target as Node) || captionLabel.contains(event.target as Node),
    update(nextNode) {
      if (nextNode.type.name !== "kosh_file_attachment") return false;
      node = nextNode;
      render();
      return true;
    },
  };
}

function button(label: string, action: () => void): HTMLButtonElement {
  const element = document.createElement("button");
  element.className = "kosh-file-node__button";
  element.type = "button";
  element.textContent = label;
  element.addEventListener("click", action);
  return element;
}

function statusText(attrs: Record<string, unknown>): string {
  const size = formatBytes(Number(attrs.byteLength));
  const mediaType = String(attrs.mediaType || "application/octet-stream");
  switch (attrs.extractionStatus) {
    case "READY": {
      const lines = Number(attrs.extractedLineCount);
      return `${size} · ${mediaType} · ${lines} line${lines === 1 ? "" : "s"} searchable`;
    }
    case "FAILED":
      return `${size} · ${String(attrs.extractionError || "Text extraction failed")}`;
    default:
      return `${size} · ${mediaType} · Content not searchable`;
  }
}

function formatBytes(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return "Unknown size";
  if (value < 1_024) return `${value} B`;
  if (value < 1_024 * 1_024) return `${(value / 1_024).toFixed(value < 10_240 ? 1 : 0)} KB`;
  return `${(value / (1_024 * 1_024)).toFixed(1)} MB`;
}

function fileIcon(filename: string): string {
  const extension = filename.split(".").pop();
  if (!extension || extension === filename || extension.length > 5) return "FILE";
  return extension.toUpperCase();
}

function reportError(error: unknown) {
  console.error("Attachment action failed", error);
}

import type { ImageStatusRecord } from "../backend/contracts";
import { attachmentMediaUrl } from "../media/gateway";
import type { Node as ProseMirrorNode } from "prosemirror-model";
import { NodeSelection } from "prosemirror-state";
import type { EditorView, NodeView } from "prosemirror-view";
import { KOSH_EDITOR_EDITABLE_EVENT } from "./editorEvents";

export const MIN_IMAGE_WIDTH_PERCENT = 10;
export const MAX_IMAGE_WIDTH_PERCENT = 100;
const STATUS_POLL_MS = 1_500;
const RETRY_STATUS_MAX_POLL_MS = 5 * 60_000;

export interface ImageNodeViewActions {
  loadStatus?: (attachmentId: string) => Promise<ImageStatusRecord>;
  retryOcr?: (attachmentId: string) => Promise<ImageStatusRecord>;
}

export function imageNodeView(
  initialNode: ProseMirrorNode,
  view: EditorView,
  getPos: () => number | undefined,
  actions: ImageNodeViewActions,
): NodeView {
  let node = initialNode;
  let active = true;
  let pollTimer: number | undefined;
  let statusRequest = 0;

  const dom = document.createElement("figure");
  dom.className = "kosh-editor-image";
  dom.contentEditable = "false";
  dom.dataset.koshImage = "true";

  const image = document.createElement("img");
  image.draggable = false;
  dom.append(image);

  const resizeHandle = document.createElement("span");
  resizeHandle.className = "kosh-editor-image__resize-handle";
  resizeHandle.setAttribute("aria-hidden", "true");
  dom.append(resizeHandle);

  const fields = document.createElement("div");
  fields.className = "kosh-editor-image__fields";

  const altLabel = document.createElement("label");
  altLabel.textContent = "Alt text";
  const altInput = document.createElement("input");
  altInput.className = "kosh-editor-image__alt";
  altInput.maxLength = 500;
  altInput.placeholder = "Describe this image";
  altLabel.append(altInput);
  fields.append(altLabel);

  const captionLabel = document.createElement("label");
  captionLabel.textContent = "Caption";
  const captionInput = document.createElement("input");
  captionInput.className = "kosh-editor-image__caption";
  captionInput.maxLength = 2_000;
  captionInput.placeholder = "Optional caption";
  captionLabel.append(captionInput);
  fields.append(captionLabel);
  dom.append(fields);

  const footer = document.createElement("div");
  footer.className = "kosh-editor-image__footer";
  const status = document.createElement("span");
  status.className = "kosh-editor-image__status";
  status.setAttribute("aria-live", "polite");
  footer.append(status);

  const retry = document.createElement("button");
  retry.className = "kosh-editor-image__action";
  retry.textContent = "Retry text recognition";
  retry.type = "button";
  retry.hidden = true;
  footer.append(retry);

  const remove = document.createElement("button");
  remove.className = "kosh-editor-image__action";
  remove.textContent = "Remove";
  remove.type = "button";
  footer.append(remove);
  dom.append(footer);

  const applyNode = (nextNode: ProseMirrorNode) => {
    node = nextNode;
    dom.style.width = `${clampImageWidth(Number(node.attrs.widthPercent))}%`;
    image.src = attachmentMediaUrl(String(node.attrs.attachmentId));
    image.alt = String(node.attrs.altText ?? "");
    altInput.value = String(node.attrs.altText ?? "");
    captionInput.value = String(node.attrs.caption ?? "");
    dom.setAttribute(
      "aria-label",
      node.attrs.altText ? `Image: ${String(node.attrs.altText)}` : "Tidbit image",
    );
    const editable = view.editable;
    altInput.disabled = !editable;
    captionInput.disabled = !editable;
    remove.disabled = !editable;
    retry.disabled = !editable;
  };
  const handleEditableChange = () => applyNode(node);
  view.dom.addEventListener(KOSH_EDITOR_EDITABLE_EVENT, handleEditableChange);
  applyNode(node);

  const updateAttributes = (attributes: Record<string, unknown>) => {
    if (!view.editable) {
      return;
    }
    const position = getPos();
    if (position === undefined) {
      return;
    }
    view.dispatch(
      view.state.tr.setNodeMarkup(position, undefined, {
        ...node.attrs,
        ...attributes,
      }),
    );
  };

  const commitAlt = () => {
    const altText = altInput.value;
    if (altText !== node.attrs.altText) {
      updateAttributes({ altText });
    }
  };
  const commitCaption = () => {
    const caption = captionInput.value;
    if (caption !== node.attrs.caption) {
      updateAttributes({ caption });
    }
  };
  altInput.addEventListener("input", commitAlt);
  captionInput.addEventListener("input", commitCaption);

  const removeImage = () => {
    if (!view.editable) {
      return;
    }
    const position = getPos();
    if (position === undefined) {
      return;
    }
    view.dispatch(view.state.tr.delete(position, position + node.nodeSize).scrollIntoView());
    view.focus();
  };
  remove.addEventListener("click", removeImage);

  const renderStatus = (record: ImageStatusRecord) => {
    retry.hidden = true;
    retry.disabled = !view.editable;
    status.title = record.ocrError ?? "";
    switch (record.ocrStatus) {
      case "PENDING":
        status.textContent = "Text recognition queued";
        break;
      case "RUNNING":
        status.textContent = "Recognizing text…";
        break;
      case "RETRY_WAIT":
        status.textContent = "Text recognition will retry";
        break;
      case "READY":
        status.textContent = "Image text indexed";
        break;
      case "FAILED":
        status.textContent = record.ocrError ?? "Text recognition failed";
        retry.hidden = !actions.retryOcr;
        break;
    }
  };

  const loadStatus = async () => {
    if (!active || !actions.loadStatus) {
      return;
    }
    const request = ++statusRequest;
    try {
      const record = await actions.loadStatus(String(node.attrs.attachmentId));
      if (!active || request !== statusRequest) {
        return;
      }
      renderStatus(record);
      if (["PENDING", "RUNNING", "RETRY_WAIT"].includes(record.ocrStatus)) {
        pollTimer = window.setTimeout(() => void loadStatus(), statusPollDelay(record));
      }
    } catch {
      if (active && request === statusRequest) {
        status.textContent = "Image status unavailable";
      }
    }
  };

  const retryRecognition = () => {
    if (!view.editable || !actions.retryOcr) {
      return;
    }
    retry.disabled = true;
    status.textContent = "Requeueing text recognition…";
    void actions
      .retryOcr(String(node.attrs.attachmentId))
      .then((record) => {
        if (!active) {
          return;
        }
        renderStatus(record);
        void loadStatus();
      })
      .catch(() => {
        if (active) {
          status.textContent = "Could not retry text recognition";
          retry.disabled = false;
          retry.hidden = false;
        }
      });
  };
  retry.addEventListener("click", retryRecognition);

  const beginResize = (event: PointerEvent) => {
    if (!view.editable || event.button !== 0) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    const parentWidth = dom.parentElement?.getBoundingClientRect().width ?? 0;
    if (parentWidth <= 0) {
      return;
    }
    const startX = event.clientX;
    const startWidth = Number(node.attrs.widthPercent);
    let nextWidth = startWidth;
    dom.classList.add("kosh-editor-image--resizing");
    resizeHandle.setPointerCapture?.(event.pointerId);

    const move = (moveEvent: PointerEvent) => {
      nextWidth = clampImageWidth(startWidth + ((moveEvent.clientX - startX) / parentWidth) * 100);
      dom.style.width = `${nextWidth}%`;
    };
    const finish = (finishEvent: PointerEvent) => {
      resizeHandle.removeEventListener("pointermove", move);
      resizeHandle.removeEventListener("pointerup", finish);
      resizeHandle.removeEventListener("pointercancel", cancel);
      resizeHandle.releasePointerCapture?.(finishEvent.pointerId);
      dom.classList.remove("kosh-editor-image--resizing");
      if (nextWidth === startWidth) {
        dom.style.width = `${startWidth}%`;
        return;
      }
      updateAttributes({ widthPercent: nextWidth });
    };
    const cancel = (cancelEvent: PointerEvent) => {
      nextWidth = startWidth;
      finish(cancelEvent);
    };
    resizeHandle.addEventListener("pointermove", move);
    resizeHandle.addEventListener("pointerup", finish);
    resizeHandle.addEventListener("pointercancel", cancel);
  };
  resizeHandle.addEventListener("pointerdown", beginResize);
  void loadStatus();

  return {
    dom,
    update(nextNode) {
      if (nextNode.type !== node.type) {
        return false;
      }
      applyNode(nextNode);
      return true;
    },
    selectNode() {
      dom.classList.add("kosh-editor-image--selected");
    },
    deselectNode() {
      dom.classList.remove("kosh-editor-image--selected");
    },
    stopEvent(event) {
      return (
        fields.contains(event.target as Node) ||
        footer.contains(event.target as Node) ||
        resizeHandle.contains(event.target as Node)
      );
    },
    destroy() {
      active = false;
      statusRequest += 1;
      if (pollTimer !== undefined) {
        window.clearTimeout(pollTimer);
      }
      altInput.removeEventListener("input", commitAlt);
      captionInput.removeEventListener("input", commitCaption);
      remove.removeEventListener("click", removeImage);
      retry.removeEventListener("click", retryRecognition);
      resizeHandle.removeEventListener("pointerdown", beginResize);
      view.dom.removeEventListener(KOSH_EDITOR_EDITABLE_EVENT, handleEditableChange);
    },
  };
}

export function statusPollDelay(
  record: Pick<ImageStatusRecord, "nextAttemptAtMs" | "ocrStatus">,
  nowMs = Date.now(),
): number {
  if (record.ocrStatus !== "RETRY_WAIT") {
    return STATUS_POLL_MS;
  }
  const untilRetry =
    record.nextAttemptAtMs === null ? RETRY_STATUS_MAX_POLL_MS : record.nextAttemptAtMs - nowMs;
  return Math.max(STATUS_POLL_MS, Math.min(RETRY_STATUS_MAX_POLL_MS, untilRetry));
}

export function pendingImageNodeView(initialNode: ProseMirrorNode): NodeView {
  const dom = document.createElement("div");
  dom.className = "kosh-editor-image-pending";
  dom.contentEditable = "false";
  dom.setAttribute("aria-label", String(initialNode.attrs.label));
  dom.setAttribute("role", "status");
  dom.textContent = `${String(initialNode.attrs.label)}…`;
  return { dom };
}

export function resizeSelectedImage(delta: number) {
  return (state: EditorView["state"], dispatch?: EditorView["dispatch"]): boolean => {
    if (!(state.selection instanceof NodeSelection)) {
      return false;
    }
    const node = state.selection.node;
    if (node.type.name !== "kosh_image") {
      return false;
    }
    const widthPercent = clampImageWidth(Number(node.attrs.widthPercent) + delta);
    if (widthPercent === node.attrs.widthPercent) {
      return true;
    }
    dispatch?.(
      state.tr.setNodeMarkup(state.selection.from, undefined, {
        ...node.attrs,
        widthPercent,
      }),
    );
    return true;
  };
}

export function initialImageWidth(naturalWidth: number, editorWidth: number): number {
  if (editorWidth <= 0 || naturalWidth >= editorWidth) {
    return MAX_IMAGE_WIDTH_PERCENT;
  }
  return clampImageWidth((naturalWidth / editorWidth) * 100);
}

export function clampImageWidth(value: number): number {
  return Math.max(MIN_IMAGE_WIDTH_PERCENT, Math.min(MAX_IMAGE_WIDTH_PERCENT, Math.round(value)));
}

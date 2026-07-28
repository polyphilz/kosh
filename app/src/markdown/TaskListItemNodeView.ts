import type { Node as ProseMirrorNode } from "prosemirror-model";
import type { EditorView, NodeView, ViewMutationRecord } from "prosemirror-view";
import { KOSH_EDITOR_EDITABLE_EVENT } from "./editorEvents";

export function taskListItemNodeView(
  node: ProseMirrorNode,
  view: EditorView,
  getPos: () => number | undefined,
): NodeView {
  return new TaskListItemView(node, view, getPos);
}

class TaskListItemView implements NodeView {
  readonly contentDOM: HTMLElement;
  readonly dom: HTMLElement;
  private readonly checkbox: HTMLInputElement | null;
  private readonly getPos: () => number | undefined;
  private readonly outerView: EditorView;
  private node: ProseMirrorNode;

  constructor(node: ProseMirrorNode, view: EditorView, getPos: () => number | undefined) {
    this.node = node;
    this.outerView = view;
    this.getPos = getPos;
    this.dom = document.createElement("li");
    this.contentDOM = document.createElement("div");
    this.contentDOM.className = "kosh-task-list-item__content";
    const checked = taskState(node);
    this.checkbox = checked === null ? null : this.createCheckbox();
    if (this.checkbox) {
      this.dom.append(this.checkbox);
    }
    this.dom.append(this.contentDOM);
    this.renderState();
    this.outerView.dom.addEventListener(KOSH_EDITOR_EDITABLE_EVENT, this.handleEditableChange);
  }

  update = (node: ProseMirrorNode): boolean => {
    if (node.type !== this.node.type || (taskState(node) === null) !== (this.checkbox === null)) {
      return false;
    }
    this.node = node;
    this.renderState();
    return true;
  };

  stopEvent = (event: Event): boolean => event.target === this.checkbox;

  ignoreMutation = (mutation: ViewMutationRecord): boolean => mutation.target === this.checkbox;

  destroy = () => {
    this.outerView.dom.removeEventListener(KOSH_EDITOR_EDITABLE_EVENT, this.handleEditableChange);
  };

  private createCheckbox(): HTMLInputElement {
    const checkbox = document.createElement("input");
    checkbox.className = "kosh-task-list-item__checkbox";
    checkbox.contentEditable = "false";
    checkbox.type = "checkbox";
    checkbox.addEventListener("change", this.handleChange);
    return checkbox;
  }

  private handleChange = () => {
    if (!this.checkbox) {
      return;
    }
    if (!this.outerView.editable) {
      this.renderState();
      return;
    }
    const position = this.getPos();
    if (position === undefined) {
      this.renderState();
      return;
    }
    this.outerView.dispatch(
      this.outerView.state.tr.setNodeMarkup(position, undefined, {
        ...this.node.attrs,
        checked: this.checkbox.checked,
      }),
    );
    this.outerView.focus();
  };

  private handleEditableChange = () => this.renderState();

  private renderState() {
    const checked = taskState(this.node);
    if (checked === null || !this.checkbox) {
      this.dom.className = "";
      this.dom.removeAttribute("data-checked");
      this.dom.removeAttribute("data-task-item");
      return;
    }
    this.dom.className = "kosh-task-list-item";
    this.dom.dataset.checked = String(checked);
    this.dom.dataset.taskItem = "true";
    this.checkbox.checked = checked;
    this.checkbox.disabled = !this.outerView.editable;
    this.checkbox.setAttribute(
      "aria-label",
      checked ? "Mark task incomplete" : "Mark task complete",
    );
  }
}

function taskState(node: ProseMirrorNode): boolean | null {
  return typeof node.attrs.checked === "boolean" ? node.attrs.checked : null;
}

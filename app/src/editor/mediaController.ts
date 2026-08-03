import type { SelectedAttachmentRecord } from "../backend/contracts";
import { selectedAttachmentToMediaBlock } from "./mediaBlocks";
import type { KoshBlockNoteEditor, KoshBlockNotePartialBlock } from "./schema";

export interface BlockNoteMediaControllerOptions {
  editorWidth?: () => number;
  onError?: (error: unknown) => void;
  onPendingChange?: (pending: boolean) => void;
}

export interface BlockNoteMediaController {
  activate(): void;
  begin(label: string, ingest: () => Promise<SelectedAttachmentRecord | null>): Promise<void>;
  dispose(): void;
  handleImagePaste(
    event: Pick<ClipboardEvent, "clipboardData" | "preventDefault">,
    ingest: () => Promise<SelectedAttachmentRecord | null>,
  ): boolean;
  insert(records: readonly SelectedAttachmentRecord[]): void;
  pending(): boolean;
}

export function createBlockNoteMediaController(
  editor: KoshBlockNoteEditor,
  options: BlockNoteMediaControllerOptions = {},
): BlockNoteMediaController {
  const pendingIds = new Set<string>();
  let disposed = false;

  const notifyPending = () => options.onPendingChange?.(pendingIds.size > 0);
  const finish = (blockId: string) => {
    pendingIds.delete(blockId);
    notifyPending();
  };
  const editorWidth = () => options.editorWidth?.() ?? editor.domElement?.clientWidth ?? 0;

  const insert = (records: readonly SelectedAttachmentRecord[]) => {
    if (disposed || records.length === 0) return;
    const reference = currentReference(editor);
    const inserted = editor.insertBlocks(
      records.map((record) =>
        selectedAttachmentToMediaBlock(record, editorWidth()),
      ) as KoshBlockNotePartialBlock[],
      reference,
      "after",
    );
    focusAfterMedia(editor, inserted.at(-1));
  };

  const begin = async (label: string, ingest: () => Promise<SelectedAttachmentRecord | null>) => {
    if (disposed) return;
    const requestId = crypto.randomUUID();
    const insertion = insertPendingAtCursor(editor, {
      type: "koshPendingMedia",
      props: { label, requestId },
    });
    const pendingBlock = insertion?.block;
    if (!pendingBlock) throw new Error("BlockNote did not insert the pending media block");
    pendingIds.add(pendingBlock.id);
    notifyPending();
    try {
      const record = await ingest();
      if (disposed) return;
      const current = editor.getBlock(pendingBlock.id);
      if (!current) return;
      if (!record) {
        removePendingAndRollback(editor, current, insertion.split);
        return;
      }
      editor.replaceBlocks(
        [current],
        [selectedAttachmentToMediaBlock(record, editorWidth()) as KoshBlockNotePartialBlock],
      );
    } catch (error) {
      if (!disposed) {
        const current = editor.getBlock(pendingBlock.id);
        if (current) removePendingAndRollback(editor, current, insertion.split);
        options.onError?.(error);
      }
    } finally {
      finish(pendingBlock.id);
    }
  };

  return {
    activate() {
      disposed = false;
    },
    begin,
    dispose() {
      disposed = true;
      pendingIds.clear();
      notifyPending();
    },
    handleImagePaste(event, ingest) {
      const hasImage = [...(event.clipboardData?.items ?? [])].some((item) =>
        item.type.startsWith("image/"),
      );
      if (!hasImage || disposed) return false;
      event.preventDefault();
      void begin("Processing pasted image", ingest);
      return true;
    },
    insert,
    pending: () => pendingIds.size > 0,
  };
}

function currentReference(editor: KoshBlockNoteEditor) {
  try {
    return editor.getTextCursorPosition().block;
  } catch {
    const reference = editor.document.at(-1);
    if (!reference) throw new Error("BlockNote editor document is empty");
    return reference;
  }
}

function insertPendingAtCursor(editor: KoshBlockNoteEditor, pending: KoshBlockNotePartialBlock) {
  let selection = editor._tiptapEditor.state.selection;
  if (!selection.empty) {
    if (!editor._tiptapEditor.commands.setTextSelection(selection.to)) {
      throw new Error("BlockNote could not preserve the selected text before media insertion");
    }
    selection = editor._tiptapEditor.state.selection;
  }
  const reference = currentReference(editor);
  const isTextCursor = selection.$from.parent.isTextblock;
  const isAtStart = selection.empty && selection.$from.parentOffset === 0;
  const isAtEnd =
    selection.empty && selection.$from.parentOffset === selection.$from.parent.content.size;

  if (isTextCursor && isAtStart) {
    const [inserted] = editor.insertBlocks([pending], reference, "before");
    editor.setTextCursorPosition(reference, "start");
    editor.focus();
    return inserted ? { block: inserted } : undefined;
  }

  if (isTextCursor && !isAtEnd) {
    const split = editor._tiptapEditor.commands.keyboardShortcut("Enter");
    if (split) {
      const tail = currentReference(editor);
      const [inserted] = editor.insertBlocks([pending], tail, "before");
      editor.setTextCursorPosition(tail, "start");
      editor.focus();
      return inserted
        ? { block: inserted, split: { headId: reference.id, tailId: tail.id } }
        : undefined;
    }
  }

  const [inserted] = editor.insertBlocks([pending], reference, "after");
  focusAfterMedia(editor, inserted);
  return inserted ? { block: inserted } : undefined;
}

function removePendingAndRollback(
  editor: KoshBlockNoteEditor,
  pending: { id: string },
  split: { headId: string; tailId: string } | undefined,
) {
  const activeBlockId = currentReference(editor).id;
  editor.removeBlocks([pending]);
  if (!split) return;
  const head = editor.getBlock(split.headId);
  const tail = editor.getBlock(split.tailId);
  if (
    !head ||
    !tail ||
    editor.getNextBlock(head)?.id !== tail.id ||
    !Array.isArray(head.content) ||
    !Array.isArray(tail.content)
  ) {
    return;
  }
  editor.updateBlock(head, {
    content: [...head.content, ...tail.content],
  } as KoshBlockNotePartialBlock);
  editor.removeBlocks([tail]);
  if (activeBlockId === head.id || activeBlockId === tail.id) {
    editor.setTextCursorPosition(head, "end");
    editor.focus();
  }
}

function focusAfterMedia(editor: KoshBlockNoteEditor, media: { id: string } | undefined) {
  if (!media) return;
  const current = editor.getBlock(media.id);
  if (!current) return;
  const next = editor.getNextBlock(current);
  const focusTarget =
    next?.type === "paragraph"
      ? next
      : editor.insertBlocks([{ type: "paragraph" }], current, "after")[0];
  if (focusTarget) editor.setTextCursorPosition(focusTarget, "start");
  editor.focus();
}

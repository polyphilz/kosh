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
  track<T>(operation: () => Promise<T>): Promise<T>;
}

interface SplitTransaction {
  committed: boolean;
  headId: string;
  pendingIds: Set<string>;
  tailId: string;
}

export function createBlockNoteMediaController(
  editor: KoshBlockNoteEditor,
  options: BlockNoteMediaControllerOptions = {},
): BlockNoteMediaController {
  const pendingIds = new Set<string>();
  const splitTransactions = new Set<SplitTransaction>();
  const splitTransactionByPendingId = new Map<string, SplitTransaction>();
  let disposed = false;

  const notifyPending = () => options.onPendingChange?.(pendingIds.size > 0);
  const finish = (blockId: string) => {
    pendingIds.delete(blockId);
    notifyPending();
  };
  const editorWidth = () => options.editorWidth?.() ?? editor.domElement?.clientWidth ?? 0;

  const insert = (records: readonly SelectedAttachmentRecord[]) => {
    if (disposed || !editor.isEditable || records.length === 0) return;
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
    if (disposed || !editor.isEditable) return;
    const requestId = crypto.randomUUID();
    const insertion = insertPendingAtCursor(editor, {
      type: "koshPendingMedia",
      props: { label, requestId },
    });
    const pendingBlock = insertion?.block;
    if (!pendingBlock) throw new Error("BlockNote did not insert the pending media block");
    registerSplitTransaction(
      editor,
      pendingBlock.id,
      insertion.split,
      splitTransactions,
      splitTransactionByPendingId,
    );
    pendingIds.add(pendingBlock.id);
    notifyPending();
    try {
      const record = await ingest();
      if (disposed) return;
      const current = editor.getBlock(pendingBlock.id);
      if (!current) return;
      if (!record) {
        removePendingAndRollback(
          editor,
          current,
          settleSplitTransaction(
            pendingBlock.id,
            false,
            splitTransactions,
            splitTransactionByPendingId,
          ),
        );
        return;
      }
      editor.replaceBlocks(
        [current],
        [selectedAttachmentToMediaBlock(record, editorWidth()) as KoshBlockNotePartialBlock],
      );
      settleSplitTransaction(pendingBlock.id, true, splitTransactions, splitTransactionByPendingId);
    } catch (error) {
      if (!disposed) {
        const current = editor.getBlock(pendingBlock.id);
        const split = settleSplitTransaction(
          pendingBlock.id,
          false,
          splitTransactions,
          splitTransactionByPendingId,
        );
        if (current) removePendingAndRollback(editor, current, split);
        options.onError?.(error);
      }
    } finally {
      finish(pendingBlock.id);
    }
  };

  const track = async <T>(operation: () => Promise<T>): Promise<T> => {
    if (disposed) throw new Error("the media controller is disposed");
    const requestId = crypto.randomUUID();
    pendingIds.add(requestId);
    notifyPending();
    try {
      return await operation();
    } finally {
      finish(requestId);
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
      splitTransactions.clear();
      splitTransactionByPendingId.clear();
      notifyPending();
    },
    handleImagePaste(event, ingest) {
      const hasImage = [...(event.clipboardData?.items ?? [])].some((item) =>
        item.type.startsWith("image/"),
      );
      if (!hasImage || disposed || !editor.isEditable) return false;
      event.preventDefault();
      void begin("Processing pasted image", ingest);
      return true;
    },
    insert,
    pending: () => pendingIds.size > 0,
    track,
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
  split: SplitTransaction | undefined,
) {
  const activeBlockId = currentReference(editor).id;
  editor.removeBlocks([pending]);
  if (!split || split.committed || split.pendingIds.size > 0) return;
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

function registerSplitTransaction(
  editor: KoshBlockNoteEditor,
  pendingId: string,
  split: { headId: string; tailId: string } | undefined,
  transactions: Set<SplitTransaction>,
  byPendingId: Map<string, SplitTransaction>,
) {
  const transaction = split
    ? { ...split, committed: false, pendingIds: new Set<string>() }
    : [...transactions].find((candidate) => blockIsBetween(editor, pendingId, candidate));
  if (!transaction) return;
  if (split) transactions.add(transaction);
  transaction.pendingIds.add(pendingId);
  byPendingId.set(pendingId, transaction);
}

function settleSplitTransaction(
  pendingId: string,
  committed: boolean,
  transactions: Set<SplitTransaction>,
  byPendingId: Map<string, SplitTransaction>,
): SplitTransaction | undefined {
  const transaction = byPendingId.get(pendingId);
  if (!transaction) return undefined;
  byPendingId.delete(pendingId);
  transaction.pendingIds.delete(pendingId);
  transaction.committed ||= committed;
  if (transaction.pendingIds.size === 0) transactions.delete(transaction);
  return transaction;
}

function blockIsBetween(
  editor: KoshBlockNoteEditor,
  blockId: string,
  split: Pick<SplitTransaction, "headId" | "tailId">,
): boolean {
  let block = editor.getBlock(split.headId);
  const visited = new Set<string>();
  while (block && !visited.has(block.id)) {
    visited.add(block.id);
    block = editor.getNextBlock(block);
    if (!block || block.id === split.tailId) return false;
    if (block.id === blockId) return true;
  }
  return false;
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

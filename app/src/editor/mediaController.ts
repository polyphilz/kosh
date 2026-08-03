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
    const pendingBlock = insertPendingAtCursor(editor, {
      type: "koshPendingMedia",
      props: { label, requestId },
    });
    if (!pendingBlock) throw new Error("BlockNote did not insert the pending media block");
    pendingIds.add(pendingBlock.id);
    notifyPending();
    try {
      const record = await ingest();
      if (disposed) return;
      const current = editor.getBlock(pendingBlock.id);
      if (!current) return;
      if (!record) {
        editor.removeBlocks([current]);
        return;
      }
      editor.replaceBlocks(
        [current],
        [selectedAttachmentToMediaBlock(record, editorWidth()) as KoshBlockNotePartialBlock],
      );
    } catch (error) {
      if (!disposed) {
        const current = editor.getBlock(pendingBlock.id);
        if (current) editor.removeBlocks([current]);
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
  const reference = currentReference(editor);
  const selection = editor._tiptapEditor.state.selection;
  const isTextCursor = selection.$from.parent.isTextblock;
  const isAtStart = selection.empty && selection.$from.parentOffset === 0;
  const isAtEnd =
    selection.empty && selection.$from.parentOffset === selection.$from.parent.content.size;

  if (isTextCursor && isAtStart) {
    const [inserted] = editor.insertBlocks([pending], reference, "before");
    editor.setTextCursorPosition(reference, "start");
    editor.focus();
    return inserted;
  }

  if (isTextCursor && !isAtEnd) {
    const split = editor._tiptapEditor.commands.keyboardShortcut("Enter");
    if (split) {
      const tail = currentReference(editor);
      const [inserted] = editor.insertBlocks([pending], tail, "before");
      editor.setTextCursorPosition(tail, "start");
      editor.focus();
      return inserted;
    }
  }

  const [inserted] = editor.insertBlocks([pending], reference, "after");
  focusAfterMedia(editor, inserted);
  return inserted;
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

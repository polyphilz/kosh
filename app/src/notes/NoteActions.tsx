import { useEffect, useState } from "react";
import { Button } from "../components/Button";
import { Dialog } from "../components/Dialog";
import { KoshText } from "../components/KoshText";
import { KoshTextTone, KoshTextVariant } from "../components/kosh-text-types";
import { keyboardEventMatchesAccelerator } from "../shortcuts/localShortcuts";

interface NoteActionsProps {
  canDelete: boolean;
  deleteError: string | null;
  deleteShortcut?: string;
  deleteOpen?: boolean;
  deleting: boolean;
  disabled: boolean;
  onDelete: () => void;
  onDeleteOpenChange?: (open: boolean) => void;
}

export function NoteActions({
  canDelete,
  deleteError,
  deleteShortcut,
  deleteOpen: controlledDeleteOpen,
  deleting,
  disabled,
  onDelete,
  onDeleteOpenChange,
}: NoteActionsProps) {
  const [uncontrolledDeleteOpen, setUncontrolledDeleteOpen] = useState(false);
  const deleteOpen = controlledDeleteOpen ?? uncontrolledDeleteOpen;
  const setDeleteOpen = (open: boolean) => {
    setUncontrolledDeleteOpen(open);
    onDeleteOpenChange?.(open);
  };

  useEffect(() => {
    if (!deleteShortcut) return;
    const openDeleteDialog = (event: KeyboardEvent) => {
      if (
        !canDelete ||
        disabled ||
        deleting ||
        deleteOpen ||
        event.isComposing ||
        event.repeat ||
        !keyboardEventMatchesAccelerator(event, deleteShortcut) ||
        document.querySelector('[aria-modal="true"]')
      ) {
        return;
      }
      event.preventDefault();
      event.stopImmediatePropagation();
      setDeleteOpen(true);
    };
    window.addEventListener("keydown", openDeleteDialog, true);
    return () => window.removeEventListener("keydown", openDeleteDialog, true);
  }, [canDelete, deleteOpen, deleteShortcut, deleting, disabled]);

  return (
    <div className="note-actions">
      <Dialog
        description="The note leaves search. You can undo immediately."
        footer={
          <>
            <Button disabled={deleting} onClick={() => setDeleteOpen(false)}>
              Cancel
            </Button>
            <Button
              className="note-delete-confirm"
              disabled={deleting}
              onClick={onDelete}
              variant="danger"
            >
              {deleting ? "Deleting…" : "Delete note"}
            </Button>
          </>
        }
        onClose={() => {
          if (!deleting) setDeleteOpen(false);
        }}
        initialFocus="panel"
        open={deleteOpen}
        title="Delete this note?"
      >
        <KoshText
          as="p"
          className="note-delete-copy"
          tone={KoshTextTone.Muted}
          variant={KoshTextVariant.Body}
        >
          Its revisions and attachments remain recoverable.
        </KoshText>
        {deleteError && (
          <KoshText
            as="p"
            className="note-delete-error"
            role="alert"
            tone={KoshTextTone.Danger}
            variant={KoshTextVariant.Supporting}
          >
            {deleteError}
          </KoshText>
        )}
      </Dialog>
    </div>
  );
}

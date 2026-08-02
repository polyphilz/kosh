import { useCallback, useEffect, useRef, useState, type KeyboardEvent } from "react";
import { TidbitComposer, type TidbitComposerHandle } from "../routes/TidbitComposer";
import { quickAddNative, type QuickAddNative } from "./native";

interface QuickAddWindowProps {
  native?: QuickAddNative;
}

export function QuickAddWindow({ native = quickAddNative }: QuickAddWindowProps) {
  const composerRef = useRef<TidbitComposerHandle>(null);
  const [generation, setGeneration] = useState(0);

  const focusComposer = useCallback(() => {
    requestAnimationFrame(() => composerRef.current?.focusPrimary());
  }, []);

  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window) && native === quickAddNative) {
      focusComposer();
      return;
    }
    let active = true;
    let unlisten: (() => void) | undefined;
    void native
      .onShown(focusComposer)
      .then((stop) => {
        if (active) unlisten = stop;
        else stop();
      })
      .catch((reason: unknown) => console.error("Could not observe quick-add activation", reason));
    return () => {
      active = false;
      unlisten?.();
    };
  }, [focusComposer, native]);

  const finish = async () => {
    setGeneration((value) => value + 1);
    await native.dismiss();
  };

  const handleEscapeCapture = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key !== "Escape" || event.nativeEvent.isComposing) return;
    const target = event.target;
    if (target instanceof Element && target.closest('[role="dialog"]')) return;
    if (composerRef.current?.isEditorOverlayOpen()) return;
    event.preventDefault();
    event.stopPropagation();
    composerRef.current?.requestCancel();
  };

  return (
    <main aria-label="Quick add" className="quick-add-shell" onKeyDownCapture={handleEscapeCapture}>
      <section className="quick-add-card">
        <header className="quick-add-card__header">
          <div>
            <p className="page-kicker">Capture from anywhere</p>
            <h1>Quick add</h1>
          </div>
          <span>⌘↵ save · Esc cancel</span>
        </header>
        <TidbitComposer
          autoFocus
          contextKey="quick-add"
          key={generation}
          onCancel={() => finish()}
          onFileDialogOpenChange={(open) =>
            native
              .setFileDialogOpen(open)
              .catch((reason: unknown) =>
                console.error("Could not update quick-add file-dialog state", reason),
              )
          }
          onSaved={() => finish()}
          ref={composerRef}
          variant="quick"
        />
      </section>
    </main>
  );
}

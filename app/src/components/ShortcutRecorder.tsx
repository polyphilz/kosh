import { useEffect, useRef, useState } from "react";
import { acceleratorForKeyboardEvent, formatAccelerator } from "../shortcuts/accelerator";
import { Button } from "./Button";
import { KoshText } from "./KoshText";
import { KoshTextTone, KoshTextVariant } from "./kosh-text-types";

interface ShortcutRecorderProps {
  accelerator: string;
  disabled?: boolean;
  label: string;
  onCapture: (accelerator: string) => void;
  resetToken?: number;
}

const modifierCodes = new Set([
  "AltLeft",
  "AltRight",
  "ControlLeft",
  "ControlRight",
  "MetaLeft",
  "MetaRight",
  "ShiftLeft",
  "ShiftRight",
]);

export function ShortcutRecorder({
  accelerator,
  disabled = false,
  label,
  onCapture,
  resetToken = 0,
}: ShortcutRecorderProps) {
  const [recording, setRecording] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!recording) return;
    const capture = (event: globalThis.KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopImmediatePropagation();
        setRecording(false);
        setError(null);
        requestAnimationFrame(() => buttonRef.current?.focus());
        return;
      }
      if (modifierCodes.has(event.code)) return;
      event.preventDefault();
      event.stopImmediatePropagation();
      const result = acceleratorForKeyboardEvent(event);
      if (typeof result !== "string") {
        setError(result.message);
        return;
      }
      setRecording(false);
      setError(null);
      onCapture(result);
    };
    window.addEventListener("keydown", capture, { capture: true });
    return () => window.removeEventListener("keydown", capture, { capture: true });
  }, [onCapture, recording]);

  useEffect(() => {
    setRecording(false);
    setError(null);
  }, [resetToken]);

  return (
    <div className="shortcut-recorder">
      <Button
        aria-describedby={error ? `${label}-shortcut-error` : undefined}
        aria-label={`${label}: ${formatAccelerator(accelerator)}`}
        className={recording ? "shortcut-recorder__button--recording" : undefined}
        disabled={disabled}
        onBlur={() => {
          if (recording) {
            setRecording(false);
            setError(null);
          }
        }}
        onClick={() => {
          setRecording(true);
          setError(null);
        }}
        ref={buttonRef}
        size="compact"
      >
        {recording ? "Press shortcut…" : formatAccelerator(accelerator)}
      </Button>
      {error && (
        <KoshText
          as="span"
          id={`${label}-shortcut-error`}
          role="alert"
          tone={KoshTextTone.Danger}
          variant={KoshTextVariant.Caption}
        >
          {error}
        </KoshText>
      )}
      {recording && !error && (
        <KoshText as="span" tone={KoshTextTone.Muted} variant={KoshTextVariant.Caption}>
          Press a complete shortcut · Esc to cancel
        </KoshText>
      )}
    </div>
  );
}

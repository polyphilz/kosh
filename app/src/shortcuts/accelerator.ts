const unsupportedCodes = new Set([
  "AudioVolumeDown",
  "AudioVolumeMute",
  "AudioVolumeUp",
  "CapsLock",
  "MediaPause",
  "MediaPlay",
  "MediaPlayPause",
  "MediaStop",
  "MediaTrackNext",
  "MediaTrackPrevious",
  "NumLock",
  "Pause",
  "PrintScreen",
  "ScrollLock",
]);

export function acceleratorForKeyboardEvent(
  event: Pick<KeyboardEvent, "altKey" | "code" | "ctrlKey" | "metaKey" | "shiftKey">,
): string | { message: string } {
  if (unsupportedCodes.has(event.code)) {
    return { message: "That key cannot be used as a global shortcut." };
  }
  if (!event.ctrlKey && !event.altKey && !event.metaKey && !event.shiftKey) {
    return { message: "Include at least one modifier key." };
  }
  const modifiers = [
    event.shiftKey ? "shift" : null,
    event.ctrlKey ? "control" : null,
    event.altKey ? "alt" : null,
    event.metaKey ? "super" : null,
  ].filter((value): value is string => value !== null);
  return [...modifiers, event.code].join("+");
}

export function acceleratorKeys(accelerator: string): string[] {
  const parts = accelerator.split("+");
  const key = parts.at(-1) ?? "";
  return [
    ...parts.slice(0, -1).map((part) => {
      switch (part.toLowerCase()) {
        case "control":
          return "⌃";
        case "alt":
          return "⌥";
        case "shift":
          return "⇧";
        case "super":
          return "⌘";
        default:
          return part;
      }
    }),
    key.replace(/^Key|^Digit/u, ""),
  ];
}

export function formatAccelerator(accelerator: string): string {
  return acceleratorKeys(accelerator).join("");
}

export function describeAccelerator(accelerator: string): string {
  const parts = accelerator.split("+");
  const key = (parts.at(-1) ?? "").replace(/^Key|^Digit/u, "");
  const modifiers = parts.slice(0, -1).map((part) => {
    switch (part.toLowerCase()) {
      case "control":
        return "Control";
      case "alt":
        return "Option";
      case "shift":
        return "Shift";
      case "super":
        return "Command";
      default:
        return part;
    }
  });
  return [...modifiers, key].filter(Boolean).join(" ");
}

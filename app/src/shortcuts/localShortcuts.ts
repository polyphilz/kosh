export const LocalShortcutCommand = {
  CopyNoteLink: "COPY_NOTE_LINK",
  CopyExactNoteLink: "COPY_EXACT_NOTE_LINK",
} as const;

export type LocalShortcutCommand = (typeof LocalShortcutCommand)[keyof typeof LocalShortcutCommand];

export interface LocalKeyboardBinding {
  command: LocalShortcutCommand;
  accelerator: string;
}

export const DEFAULT_COPY_NOTE_LINK_ACCELERATOR = "super+KeyL";
export const DEFAULT_COPY_EXACT_NOTE_LINK_ACCELERATOR = "shift+super+KeyL";

export const DEFAULT_LOCAL_KEYBOARD_BINDINGS: readonly LocalKeyboardBinding[] = [
  {
    command: LocalShortcutCommand.CopyNoteLink,
    accelerator: DEFAULT_COPY_NOTE_LINK_ACCELERATOR,
  },
  {
    command: LocalShortcutCommand.CopyExactNoteLink,
    accelerator: DEFAULT_COPY_EXACT_NOTE_LINK_ACCELERATOR,
  },
];

const STORAGE_KEY = "kosh.local-shortcuts.v1";
const NOTE_ID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/u;
const reservedAccelerators = new Set([
  "alt+shift+super+keyv",
  "alt+super+keyh",
  "shift+super+keyv",
  "shift+super+keyz",
  "super+bracketleft",
  "super+bracketright",
  "super+comma",
  "super+keya",
  "super+keyb",
  "super+keyc",
  "super+keyf",
  "super+keyh",
  "super+keyi",
  "super+keyk",
  "super+keym",
  "super+keyn",
  "super+keyq",
  "super+keyu",
  "super+keyv",
  "super+keyw",
  "super+keyx",
  "super+keyz",
  "super+slash",
]);

export function readLocalKeyboardBindings(
  storage: Pick<Storage, "getItem"> = window.localStorage,
): LocalKeyboardBinding[] {
  try {
    const raw = storage.getItem(STORAGE_KEY);
    if (!raw) return cloneDefaultBindings();
    const parsed: unknown = JSON.parse(raw);
    if (!isCompleteBindingSet(parsed) || validateLocalKeyboardBindings(parsed) !== null) {
      return cloneDefaultBindings();
    }
    return parsed.map((binding) => ({ ...binding }));
  } catch {
    return cloneDefaultBindings();
  }
}

export function writeLocalKeyboardBindings(
  bindings: readonly LocalKeyboardBinding[],
  storage: Pick<Storage, "setItem"> = window.localStorage,
): void {
  if (!isCompleteBindingSet(bindings)) {
    throw new Error("Local shortcuts must contain every app command exactly once.");
  }
  storage.setItem(STORAGE_KEY, JSON.stringify(bindings));
}

export function validateLocalKeyboardBindings(
  bindings: readonly LocalKeyboardBinding[],
  globalAccelerators: readonly string[] = [],
): string | null {
  if (!isCompleteBindingSet(bindings)) {
    return "Local shortcuts must contain every app command exactly once.";
  }
  const accelerators = bindings.map((binding) => binding.accelerator.toLowerCase());
  if (new Set(accelerators).size !== accelerators.length) {
    return "Two Kosh commands cannot use the same shortcut.";
  }
  const globals = new Set(globalAccelerators.map((accelerator) => accelerator.toLowerCase()));
  if (accelerators.some((accelerator) => globals.has(accelerator))) {
    return "That shortcut is already used by a global Kosh command.";
  }
  if (accelerators.some((accelerator) => reservedAccelerators.has(accelerator))) {
    return "That shortcut is reserved by another Kosh command.";
  }
  return null;
}

export function localBindingFor(
  bindings: readonly LocalKeyboardBinding[],
  command: LocalShortcutCommand,
): LocalKeyboardBinding | undefined {
  return bindings.find((binding) => binding.command === command);
}

export function keyboardEventMatchesAccelerator(
  event: Pick<KeyboardEvent, "altKey" | "code" | "ctrlKey" | "metaKey" | "shiftKey">,
  accelerator: string,
): boolean {
  const expected = accelerator.split("+");
  const code = expected.pop();
  if (!code || event.code !== code) return false;
  const modifiers = new Set(expected);
  return (
    event.altKey === modifiers.has("alt") &&
    event.ctrlKey === modifiers.has("control") &&
    event.metaKey === modifiers.has("super") &&
    event.shiftKey === modifiers.has("shift")
  );
}

export function noteLinkForLocation(href: string, exact: boolean): string {
  const url = new URL(href);
  const route = url.hash.startsWith("#") ? url.hash.slice(1) : url.hash;
  const queryIndex = route.indexOf("?");
  const pathname = queryIndex >= 0 ? route.slice(0, queryIndex) : route;
  const match = /^\/(?:new|notes)\/([^/]+)$/u.exec(pathname);
  const noteId = match?.[1]?.toLowerCase();
  if (!noteId || !NOTE_ID_PATTERN.test(noteId)) {
    throw new Error("The current page is not a linkable note.");
  }
  const link = new URL(`kosh://note/${noteId}`);
  if (exact && queryIndex >= 0) link.search = route.slice(queryIndex);
  return link.href;
}

export interface NoteDeepLinkTarget {
  noteId: string;
  passage?: string;
}

export function noteTargetForDeepLink(href: string): NoteDeepLinkTarget | null {
  let url: URL;
  try {
    url = new URL(href);
  } catch {
    return null;
  }
  if (
    url.protocol !== "kosh:" ||
    url.hostname !== "note" ||
    url.username ||
    url.password ||
    url.port ||
    url.hash
  ) {
    return null;
  }
  const segments = url.pathname.split("/").filter(Boolean);
  if (segments.length !== 1) return null;
  const noteId = segments[0]?.toLowerCase();
  if (!noteId || !NOTE_ID_PATTERN.test(noteId)) return null;
  const passages = url.searchParams.getAll("passage");
  if (passages.length > 1 || (passages[0]?.length ?? 0) > 256) return null;
  return passages[0] ? { noteId, passage: passages[0] } : { noteId };
}

function cloneDefaultBindings(): LocalKeyboardBinding[] {
  return DEFAULT_LOCAL_KEYBOARD_BINDINGS.map((binding) => ({ ...binding }));
}

function isCompleteBindingSet(value: unknown): value is LocalKeyboardBinding[] {
  if (!Array.isArray(value) || value.length !== DEFAULT_LOCAL_KEYBOARD_BINDINGS.length) {
    return false;
  }
  const commands = new Set<LocalShortcutCommand>();
  for (const binding of value) {
    if (!isRecord(binding)) return false;
    if (!Object.values(LocalShortcutCommand).includes(binding.command as LocalShortcutCommand)) {
      return false;
    }
    if (typeof binding.accelerator !== "string" || !isAccelerator(binding.accelerator)) {
      return false;
    }
    commands.add(binding.command as LocalShortcutCommand);
  }
  return commands.size === DEFAULT_LOCAL_KEYBOARD_BINDINGS.length;
}

function isAccelerator(value: string): boolean {
  const parts = value.split("+");
  const code = parts.pop();
  if (!code || parts.length === 0 || !/^[A-Za-z][A-Za-z0-9]*$/u.test(code)) return false;
  const modifiers = new Set(parts);
  return (
    modifiers.size === parts.length &&
    parts.every((part) => ["alt", "control", "shift", "super"].includes(part))
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

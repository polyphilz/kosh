import { createExtension } from "@blocknote/core";
import type { Node as ProseMirrorNode } from "prosemirror-model";
import { Plugin, PluginKey } from "prosemirror-state";
import { Decoration, DecorationSet } from "prosemirror-view";
import type { KoshBlockNoteEditor } from "./schema";

export const FIND_IN_NOTE_REQUEST_EVENT = "kosh:find-in-note";
let pendingFindInNoteRoute: string | null = null;
let pendingFindInNoteTransfer: FindInNoteTransfer | null = null;

export interface FindInNoteTransfer {
  activeIndex: number;
  query: string;
  route: string;
}

export function requestFindInNote(route: string): void {
  pendingFindInNoteRoute = route;
  window.dispatchEvent(new CustomEvent(FIND_IN_NOTE_REQUEST_EVENT, { detail: route }));
}

export function consumeFindInNoteRequest(route: string): boolean {
  if (pendingFindInNoteRoute !== route) return false;
  pendingFindInNoteRoute = null;
  return true;
}

export function clearFindInNoteRequest(route?: string): void {
  if (route === undefined || pendingFindInNoteRoute === route) pendingFindInNoteRoute = null;
}

export function transferFindInNote(route: string, query: string, activeIndex: number): void {
  pendingFindInNoteTransfer = { activeIndex, query, route };
}

export function consumeFindInNoteTransfer(route: string): FindInNoteTransfer | null {
  if (pendingFindInNoteTransfer?.route !== route) return null;
  const transfer = pendingFindInNoteTransfer;
  pendingFindInNoteTransfer = null;
  return transfer;
}

export function clearFindInNoteTransfer(route?: string): void {
  if (route === undefined || pendingFindInNoteTransfer?.route === route) {
    pendingFindInNoteTransfer = null;
  }
}

export interface FindInNoteResult {
  activeIndex: number;
  count: number;
}

interface FindPart {
  atom: boolean;
  from: number;
  to: number;
}

interface FindMatch {
  parts: FindPart[];
}

interface FindState extends FindInNoteResult {
  matches: FindMatch[];
  query: string;
}

interface FindCommand {
  activeIndex: number;
  query: string;
}

interface TextSegment extends FindPart {
  text: string;
  textEnd: number;
  textStart: number;
}

const EMPTY_FIND_STATE: FindState = {
  activeIndex: -1,
  count: 0,
  matches: [],
  query: "",
};
const FIND_IN_NOTE_KEY = new PluginKey<FindState>("kosh-find-in-note");

export const KoshFindInNoteExtension = createExtension({
  key: "koshFindInNote",
  prosemirrorPlugins: [
    new Plugin<FindState>({
      key: FIND_IN_NOTE_KEY,
      state: {
        init: () => EMPTY_FIND_STATE,
        apply(transaction, current) {
          const command = transaction.getMeta(FIND_IN_NOTE_KEY) as FindCommand | undefined;
          if (command) return buildFindState(transaction.doc, command.query, command.activeIndex);
          return transaction.docChanged && current.query
            ? buildFindState(transaction.doc, current.query, current.activeIndex)
            : current;
        },
      },
      props: {
        decorations(state) {
          const find = FIND_IN_NOTE_KEY.getState(state);
          if (!find?.matches.length) return DecorationSet.empty;
          const decorations: Decoration[] = [];
          find.matches.forEach((match, index) => {
            const attributes = {
              "data-kosh-find-active": index === find.activeIndex ? "true" : "false",
              "data-kosh-find-match": "true",
            };
            for (const part of match.parts) {
              decorations.push(
                part.atom
                  ? Decoration.node(part.from, part.to, attributes)
                  : Decoration.inline(part.from, part.to, attributes),
              );
            }
          });
          return DecorationSet.create(state.doc, decorations);
        },
      },
    }),
  ],
});

export function findInNote(
  editor: KoshBlockNoteEditor,
  query: string,
  activeIndex = 0,
): FindInNoteResult {
  return updateFindState(editor, { activeIndex, query });
}

export function moveFindInNote(
  editor: KoshBlockNoteEditor,
  direction: "next" | "previous",
): FindInNoteResult {
  const current = FIND_IN_NOTE_KEY.getState(editor.prosemirrorView.state) ?? EMPTY_FIND_STATE;
  if (!current.matches.length) return resultFrom(current);
  const delta = direction === "next" ? 1 : -1;
  const activeIndex =
    (Math.max(0, current.activeIndex) + delta + current.matches.length) % current.matches.length;
  return updateFindState(editor, { activeIndex, query: current.query });
}

export function clearFindInNote(editor: KoshBlockNoteEditor): void {
  updateFindState(editor, { activeIndex: -1, query: "" });
}

function updateFindState(editor: KoshBlockNoteEditor, command: FindCommand): FindInNoteResult {
  editor.prosemirrorView.dispatch(
    editor.prosemirrorView.state.tr.setMeta(FIND_IN_NOTE_KEY, command),
  );
  const next = FIND_IN_NOTE_KEY.getState(editor.prosemirrorView.state) ?? EMPTY_FIND_STATE;
  if (next.activeIndex >= 0) {
    window.requestAnimationFrame(() => {
      editor.domElement
        ?.closest<HTMLElement>(".kosh-blocknote-editor")
        ?.querySelector<HTMLElement>('[data-kosh-find-active="true"]')
        ?.scrollIntoView({ behavior: "instant", block: "center" });
    });
  }
  return resultFrom(next);
}

function resultFrom(state: FindState): FindInNoteResult {
  return { activeIndex: state.activeIndex, count: state.count };
}

function buildFindState(
  document: ProseMirrorNode,
  query: string,
  requestedIndex: number,
): FindState {
  if (!query) return EMPTY_FIND_STATE;
  const expression = new RegExp(escapeExpression(query), "giu");
  const matches: FindMatch[] = [];
  document.descendants((node, position) => {
    if (typeof node.attrs.id !== "string") return;
    matches.push(...findBlockMatches(node, position, expression));
  });
  const activeIndex = matches.length
    ? Math.min(Math.max(requestedIndex, 0), matches.length - 1)
    : -1;
  return { activeIndex, count: matches.length, matches, query };
}

function findBlockMatches(
  block: ProseMirrorNode,
  blockPosition: number,
  expression: RegExp,
): FindMatch[] {
  const segments: TextSegment[] = [];
  let text = "";
  block.descendants((node, position) => {
    if (node.type.name === "blockGroup") return false;
    const segmentText = searchableText(node);
    if (!segmentText) return;
    const from = blockPosition + 1 + position;
    segments.push({
      atom: !node.isText,
      from,
      text: segmentText,
      textEnd: text.length + segmentText.length,
      textStart: text.length,
      to: from + node.nodeSize,
    });
    text += segmentText;
    return node.isText ? undefined : false;
  });
  if (!text) return [];
  expression.lastIndex = 0;
  const matches: FindMatch[] = [];
  const matchKeys = new Set<string>();
  let segmentIndex = 0;
  for (const match of text.matchAll(expression)) {
    const start = match.index;
    const end = start + match[0].length;
    while (segmentIndex < segments.length && segments[segmentIndex]!.textEnd <= start) {
      segmentIndex += 1;
    }
    const parts: FindPart[] = [];
    for (let index = segmentIndex; index < segments.length; index += 1) {
      const segment = segments[index]!;
      if (segment.textStart >= end) break;
      parts.push(...matchPart(segment, start, end));
    }
    if (parts.length === 0) continue;
    if (parts.length > 1 && parts.some((part) => part.atom)) continue;
    const key = parts
      .map((part) => `${part.atom ? "atom" : "text"}:${part.from}:${part.to}`)
      .join("|");
    if (matchKeys.has(key)) continue;
    matchKeys.add(key);
    matches.push({ parts });
  }
  return matches;
}

function matchPart(segment: TextSegment, start: number, end: number): FindPart[] {
  const overlapStart = Math.max(start, segment.textStart);
  const overlapEnd = Math.min(end, segment.textEnd);
  if (overlapStart >= overlapEnd) return [];
  if (segment.atom) return [{ atom: true, from: segment.from, to: segment.to }];
  return [
    {
      atom: false,
      from: segment.from + overlapStart - segment.textStart,
      to: segment.from + overlapEnd - segment.textStart,
    },
  ];
}

function searchableText(node: ProseMirrorNode): string {
  if (node.isText) return node.text ?? "";
  if (node.type.name === "inlineMath" || node.type.name === "displayMath") {
    return stringAttributes(node, ["latex"]);
  }
  if (node.type.name === "koshImage") {
    return stringAttributes(node, ["altText", "caption"]);
  }
  if (node.type.name === "koshFileAttachment") {
    return stringAttributes(node, ["displayFilename", "caption"]);
  }
  return "";
}

function stringAttributes(node: ProseMirrorNode, names: readonly string[]): string {
  return names
    .map((name) => node.attrs[name])
    .filter((value): value is string => typeof value === "string" && value.length > 0)
    .join("\n");
}

function escapeExpression(query: string): string {
  return query.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&");
}

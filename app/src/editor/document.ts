import { markdownToKoshBlocks } from "./markdownAdapter";
import {
  supportedKoshBlockTypes,
  type KoshBlockNoteBlock,
  type KoshBlockNotePartialBlock,
} from "./schema";

export const KOSH_DOCUMENT_SCHEMA_VERSION = 1 as const;
const MAX_DOCUMENT_BYTES = 16 * 1024 * 1024;
const MAX_DOCUMENT_BLOCKS = 100_000;
const MAX_BLOCK_ID_BYTES = 256;
const MAX_BLOCK_DEPTH = 128;
const SUPPORTED_BLOCK_TYPES = new Set<string>(supportedKoshBlockTypes);

export interface KoshDocumentV1 {
  schemaVersion: typeof KOSH_DOCUMENT_SCHEMA_VERSION;
  blocks: KoshBlockNoteBlock[];
}

type SerializableBlock = KoshBlockNoteBlock | KoshBlockNotePartialBlock;

export function createEmptyKoshDocument(): string {
  return serializeKoshDocument([{ id: crypto.randomUUID(), type: "paragraph" }]);
}

export function createKoshDocumentFromMarkdown(markdown: string): string {
  return serializeKoshDocument(assignStableBlockIds(markdownToKoshBlocks(markdown)));
}

export function createKoshDocumentFromPlainText(text: string): string {
  return serializeKoshDocument([
    {
      id: crypto.randomUUID(),
      type: "paragraph",
      content: text ? [{ type: "text", text, styles: {} }] : [],
    },
  ]);
}

export function parseKoshDocument(documentJson: string): KoshBlockNotePartialBlock[] {
  if (new TextEncoder().encode(documentJson).byteLength > MAX_DOCUMENT_BYTES) {
    throw new Error("Kosh document exceeds the 16 MiB limit");
  }
  const value: unknown = JSON.parse(documentJson);
  if (!isRecord(value) || value.schemaVersion !== KOSH_DOCUMENT_SCHEMA_VERSION) {
    throw new Error(`Kosh document schemaVersion must be ${KOSH_DOCUMENT_SCHEMA_VERSION}`);
  }
  if (!Array.isArray(value.blocks) || value.blocks.length === 0) {
    throw new Error("Kosh document blocks must be a non-empty array");
  }
  const ids = new Set<string>();
  validateBlocks(value.blocks, ids, { count: 0 }, 0);
  return structuredClone(value.blocks) as KoshBlockNotePartialBlock[];
}

export function createDurableKoshDocument(documentJson: string): string {
  if (!documentJson.includes('"koshPendingMedia"')) return documentJson;

  const blocks = parseKoshDocument(documentJson);
  const durableBlocks = withoutPendingMedia(blocks);
  if (durableBlocks.length > 0) return serializeKoshDocument(durableBlocks);

  return serializeKoshDocument([
    {
      id: blocks[0]?.id ?? crypto.randomUUID(),
      type: "paragraph",
    },
  ]);
}

export function serializeKoshDocument(blocks: readonly SerializableBlock[]): string {
  if (blocks.length === 0) throw new Error("Kosh document blocks must not be empty");
  const document: KoshDocumentV1 = {
    schemaVersion: KOSH_DOCUMENT_SCHEMA_VERSION,
    blocks: structuredClone(blocks) as KoshBlockNoteBlock[],
  };
  const encoded = JSON.stringify(document);
  parseKoshDocument(encoded);
  return encoded;
}

function assignStableBlockIds(
  blocks: readonly KoshBlockNotePartialBlock[],
): KoshBlockNotePartialBlock[] {
  return blocks.map((block) => ({
    ...block,
    id: block.id ?? crypto.randomUUID(),
    children: block.children ? assignStableBlockIds(block.children) : undefined,
  })) as KoshBlockNotePartialBlock[];
}

function withoutPendingMedia(
  blocks: readonly KoshBlockNotePartialBlock[],
): KoshBlockNotePartialBlock[] {
  return blocks.flatMap((block) => {
    if (block.type === "koshPendingMedia") return [];
    if (!block.children?.length) return [block];
    return [{ ...block, children: withoutPendingMedia(block.children) }];
  }) as KoshBlockNotePartialBlock[];
}

function validateBlocks(
  blocks: readonly unknown[],
  ids: Set<string>,
  state: { count: number },
  depth: number,
): void {
  if (depth > MAX_BLOCK_DEPTH) throw new Error("Kosh document nesting exceeds 128 levels");
  for (const value of blocks) {
    state.count += 1;
    if (state.count > MAX_DOCUMENT_BLOCKS) {
      throw new Error("Kosh document exceeds 100000 blocks");
    }
    if (!isRecord(value) || typeof value.id !== "string" || value.id.length === 0) {
      throw new Error("Every Kosh block must have a non-empty id");
    }
    if (new TextEncoder().encode(value.id).byteLength > MAX_BLOCK_ID_BYTES) {
      throw new Error("Kosh block ids must contain at most 256 bytes");
    }
    if (typeof value.type !== "string" || !SUPPORTED_BLOCK_TYPES.has(value.type)) {
      throw new Error(`Kosh block type is unsupported: ${String(value.type)}`);
    }
    if (ids.has(value.id)) throw new Error(`Kosh block id is duplicated: ${value.id}`);
    ids.add(value.id);
    if (value.children !== undefined) {
      if (!Array.isArray(value.children)) throw new Error("Kosh block children must be an array");
      validateBlocks(value.children, ids, state, depth + 1);
    }
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

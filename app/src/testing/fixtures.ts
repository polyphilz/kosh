import type { Attachment, Passage, Source, Tidbit, TidbitRevision } from "../domain/types";
import { FixedClock, SequenceIdGenerator, type Clock, type IdGenerator } from "../lib/determinism";

export interface FixtureFactory {
  attachment(overrides?: Partial<Attachment>): Attachment;
  passage(overrides?: Partial<Passage>): Passage;
  source(overrides?: Partial<Source>): Source;
  tidbit(overrides?: Partial<Tidbit>): Tidbit;
  tidbitRevision(overrides?: Partial<TidbitRevision>): TidbitRevision;
}

interface FixtureDependencies {
  clock: Clock;
  ids: IdGenerator;
}

export function createFixtureFactory(
  dependencies: FixtureDependencies = {
    clock: new FixedClock(1_785_201_600_000),
    ids: new SequenceIdGenerator(),
  },
): FixtureFactory {
  const { clock, ids } = dependencies;

  return {
    attachment: (overrides = {}) => ({
      id: ids.nextId("attachment"),
      filename: "reference.pdf",
      mediaType: "application/pdf",
      byteLength: 1024,
      extractionState: "ready",
      ...overrides,
    }),
    passage: (overrides = {}) => ({
      id: ids.nextId("passage"),
      revisionId: "revision-1",
      content: "A citation-sized fixture passage.",
      locator: {
        kind: "markdown-blocks",
        startBlock: 0,
        endBlock: 0,
      },
      ...overrides,
    }),
    source: (overrides = {}) => ({
      id: ids.nextId("source"),
      label: "Fixture source",
      url: "https://example.com/source",
      ...overrides,
    }),
    tidbit: (overrides = {}) => ({
      id: ids.nextId("tidbit"),
      currentRevisionId: "revision-1",
      createdAtMs: clock.nowMs(),
      updatedAtMs: clock.nowMs(),
      deletedAtMs: null,
      ...overrides,
    }),
    tidbitRevision: (overrides = {}) => ({
      id: ids.nextId("revision"),
      tidbitId: "tidbit-1",
      title: "Fixture tidbit",
      bodyMarkdown: "Fixture body",
      createdAtMs: clock.nowMs(),
      ...overrides,
    }),
  };
}

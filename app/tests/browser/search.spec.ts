import AxeBuilder from "@axe-core/playwright";
import type { TidbitRecord } from "../../src/backend/contracts";
import type { FakeNoteInput } from "../../src/backend/fakeBackend";
import { expect, test, type Page } from "./fixtures";

test("Command-K searches locally and opens the exact current note block", async ({ page }) => {
  await page.goto("/#/");
  await expect(page.getByRole("textbox", { name: "Note" })).toBeFocused();
  const originalUrl = page.url();
  const first = await seedTidbit(page, {
    bodyMarkdown: "Tomato technique: slow simmering preserves a bright tomato sauce.",
    sources: [{ label: "Cookbook", url: "https://www.example.com/tomato" }],
  });
  const firstBlockId = documentBlockIds(first)[0]!;
  await seedTidbit(page, {
    bodyMarkdown: "Second tomato note: roast tomato halves before blending the sauce.",
    sources: [{ label: "Kitchen log", url: "https://notes.example.org/roasting" }],
  });

  await page.keyboard.press("Meta+k");
  const dialog = page.getByRole("dialog", { name: "Search notes" });
  await expect(dialog).toBeVisible();
  const search = page.getByRole("combobox", { name: "Search notes" });
  await expect(search).toBeFocused();
  expect(page.url()).toBe(originalUrl);

  await search.fill("slow simmering");
  const result = page.getByRole("option", { name: /Tomato technique/u });
  await expect(result).toBeVisible();
  await expect(page.getByText("Lexical ready", { exact: true })).toBeVisible();
  await expect(page.getByRole("checkbox", { name: "Exact" })).toHaveCount(0);
  await expect(result.locator("mark")).toHaveText([/slow/iu, /simmering/iu]);

  await search.press("Enter");
  await expect(dialog).toHaveCount(0);
  await expect(page).toHaveURL(`http://127.0.0.1:1422/#/notes/${first.id}?blockId=${firstBlockId}`);
  await expect(page.getByLabel("Search result location")).toHaveCount(0);
  await expect(page.locator('[data-kosh-search-hit="true"]')).toContainText(/slow simmering/iu);
  await expect(page.locator('[data-kosh-search-hit="true"]')).toHaveCSS(
    "animation-name",
    "kosh-search-match-flash",
  );
  expect(page.url()).not.toContain("slow");
  expect(await searchStorageKeys(page)).toEqual([]);
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
  await expect(page.locator('[data-kosh-search-hit="true"]')).toHaveCount(0, { timeout: 3_000 });
  await expect(page).toHaveURL(`http://127.0.0.1:1422/#/notes/${first.id}?blockId=${firstBlockId}`);

  await page.keyboard.press("Meta+k");
  await page.getByRole("combobox", { name: "Search notes" }).fill("slow simmering");
  await page.getByRole("option", { name: /Tomato technique/u }).click();
  await expect(page.locator('[data-kosh-search-hit="true"]')).toContainText(/slow simmering/iu);
  await expect(page).toHaveURL(`http://127.0.0.1:1422/#/notes/${first.id}?blockId=${firstBlockId}`);

  await page.getByRole("textbox", { name: "Note" }).fill("A replacement passage.");
  await expect(page.locator('[data-kosh-search-hit="true"]')).toHaveCount(0);
});

test("reduced motion leaves the selected block visibly highlighted", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.goto("/#/");
  await seedTidbit(page, {
    bodyMarkdown: "Reduced-motion evidence stays visible without an animation.",
    sources: [],
  });

  await page.keyboard.press("Meta+k");
  await page.getByRole("combobox", { name: "Search notes" }).fill("stays visible");
  await page.getByRole("option", { name: /Reduced-motion evidence/u }).click();

  const match = page.locator('[data-kosh-search-hit="true"]');
  await expect(match).toBeVisible();
  await expect(match).toHaveCSS("animation-name", "none");
  await expect(match).not.toHaveCSS("background-color", "rgba(0, 0, 0, 0)");
});

test("a block route remains addressable when checkpoint cleanup fails", async ({ page }) => {
  await page.goto("/#/");
  const note = await seedTidbit(page, {
    bodyMarkdown: "Blocked cleanup must keep this precise passage visible.",
    sources: [],
  });
  await page.keyboard.press("Meta+k");
  await page.getByRole("combobox", { name: "Search notes" }).fill("precise passage");
  const result = page.getByRole("option", { name: /Blocked cleanup/u });
  await expect(result).toBeVisible();
  await page.evaluate(async (seeded) => {
    const backend = window.__KOSH_FAKE_BACKEND__;
    if (!backend) throw new Error("fake backend is unavailable");
    await backend.saveWorkingCopy({
      noteId: seeded.id,
      baseRevisionId: seeded.currentRevisionId,
      editGeneration: 1,
      documentJson: seeded.documentJson,
      bodyMarkdown: seeded.bodyMarkdown,
      sources: seeded.sources,
    });
    backend.checkpointWorkingCopy = async () => {
      throw new Error("simulated search cleanup failure");
    };
  }, note);

  await result.click();

  const match = page.locator('[data-kosh-search-hit="true"]');
  await expect(match).toContainText("precise passage");
  await page.waitForTimeout(1_800);
  await expect(match).toHaveCount(0);
  await expect(page).toHaveURL(new RegExp(`/#/notes/${note.id}\\?blockId=`, "u"));
});

test("search checkpoints the active note before querying", async ({ page }) => {
  await page.goto("/#/");
  await page
    .getByRole("textbox", { name: "Note" })
    .fill("A just-authored quokka detail must be searchable immediately.");

  await page.keyboard.press("Meta+k");
  await page.getByRole("combobox", { name: "Search notes" }).fill("quokka detail");

  await expect(page.getByRole("option", { name: /quokka detail/u })).toBeVisible();
});

test("a route-backed search result remains on its owning note", async ({ page }) => {
  await page.goto("/#/");
  const note = await seedTidbit(page, {
    title: null,
    bodyMarkdown: "Route-backed evidence names the exact cedar passage.",
    sources: [],
  });
  await page.goto("/#/search");
  const dialog = page.getByRole("dialog", { name: "Search notes" });
  await dialog.getByRole("combobox", { name: "Search notes" }).fill("cedar passage");
  await dialog.getByRole("option", { name: /cedar passage/u }).click();

  await expect(dialog).toHaveCount(0);
  await expect(page).toHaveURL(
    `http://127.0.0.1:1422/#/notes/${note.id}?blockId=${documentBlockIds(note)[0]}`,
  );
  await expect(page.locator('[data-kosh-search-hit="true"]')).toContainText("cedar passage");
  await expect(page.getByLabel("Search result location")).toHaveCount(0);
});

test("dismissal clears transient search and stale responses cannot replace newer results", async ({
  page,
}) => {
  await page.goto("/#/");
  await seedTidbit(page, {
    bodyMarkdown: "Slow response: only the slow query should find this passage.",
    sources: [],
  });
  await seedTidbit(page, {
    bodyMarkdown: "Fast response: only the fast query should find this passage.",
    sources: [],
  });
  await page.evaluate(() => {
    const backend = window.__KOSH_FAKE_BACKEND__;
    if (!backend) throw new Error("fake backend is unavailable");
    const search = backend.searchBlocks.bind(backend);
    let releaseSlow!: () => void;
    const slowRelease = new Promise<void>((resolve) => {
      releaseSlow = resolve;
    });
    let failedOnce = false;
    backend.searchBlocks = async (input) => {
      if (input.query === "slow") {
        Reflect.set(window, "__KOSH_SLOW_SEARCH_STARTED__", true);
        await slowRelease;
        Reflect.set(window, "__KOSH_SLOW_SEARCH_COMPLETED__", true);
      }
      if (input.query === "explode" && !failedOnce) {
        failedOnce = true;
        throw new Error("controlled search failure");
      }
      return search(input);
    };
    Reflect.set(window, "__KOSH_RELEASE_SLOW_SEARCH__", releaseSlow);
  });

  await page.keyboard.press("Meta+k");
  const input = page.getByRole("combobox", { name: "Search notes" });
  await input.fill("slow");
  await expect
    .poll(() => page.evaluate(() => Reflect.get(window, "__KOSH_SLOW_SEARCH_STARTED__")))
    .toBe(true);
  await input.fill("fast");
  await expect(page.getByRole("option", { name: /Fast response/u })).toBeVisible();
  await page.evaluate(() => Reflect.get(window, "__KOSH_RELEASE_SLOW_SEARCH__")());
  await expect
    .poll(() => page.evaluate(() => Reflect.get(window, "__KOSH_SLOW_SEARCH_COMPLETED__")))
    .toBe(true);
  await expect(page.getByRole("option", { name: /Fast response/u })).toBeVisible();
  await expect(page.getByRole("option", { name: /Slow response/u })).toHaveCount(0);

  await input.fill("explode");
  await expect(page.getByRole("alert")).toContainText("controlled search failure");
  await page.getByRole("button", { name: "Try again" }).click();
  await expect(page.getByText("No matches found", { exact: true })).toBeVisible();
  await input.fill("fast");
  await expect(page.getByRole("option", { name: /Fast response/u })).toBeVisible();

  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog", { name: "Search notes" })).toHaveCount(0);
  await expect(page.getByRole("textbox", { name: "Note" })).toBeFocused();
  await page.keyboard.press("Meta+k");
  await expect(page.getByRole("combobox", { name: "Search notes" })).toHaveValue("");
  await expect(page.getByRole("option")).toHaveCount(0);
  expect(await searchStorageKeys(page)).toEqual([]);
});

test("a result edited away before navigation becomes a silent stale block link", async ({
  page,
}) => {
  await page.goto("/#/");
  const created = await seedTidbit(page, {
    bodyMarkdown: "Current evidence: the original block mentions cobalt.",
    sources: [{ label: "Lab notebook", url: "https://example.com/lab" }],
  });
  await page.keyboard.press("Meta+k");
  const search = page.getByRole("combobox", { name: "Search notes" });
  await search.fill("cobalt");
  const result = page.getByRole("option", { name: /Current evidence/u });
  await expect(result).toBeVisible();

  await page.evaluate(async (noteId) => {
    const backend = window.__KOSH_FAKE_BACKEND__;
    if (!backend) throw new Error("fake backend is unavailable");
    const current = await backend.loadTidbit(noteId);
    await backend.replaceNoteForTest({
      id: current.id,
      expectedRevisionId: current.currentRevisionId,
      bodyMarkdown: "Current evidence: the replacement block now mentions indigo.",
      sources: current.sources,
    });
  }, created.id);

  await result.click();
  await expect(page.getByRole("textbox", { name: "Note" })).toContainText(
    "the replacement block now mentions indigo.",
  );
  await expect(page).toHaveURL(`http://127.0.0.1:1422/#/notes/${created.id}`);
  await expect(page.locator('[data-kosh-search-hit="true"]')).toHaveCount(0);
});

test("file results use the attachment filename without indexing file contents", async ({
  page,
}) => {
  await page.goto("/#/");
  const attachmentId = "019f547b-6200-7000-8000-00000000d001";
  const note = await seedTidbit(page, {
    bodyMarkdown: `# Vector chapter\n\n{{kosh:attachment:${attachmentId}}}`,
    documentJson: JSON.stringify({
      schemaVersion: 1,
      blocks: [
        {
          id: "019f547b-6200-7000-8000-00000000d101",
          type: "heading",
          props: { level: 1 },
          content: [{ type: "text", text: "Vector chapter", styles: {} }],
        },
        {
          id: "019f547b-6200-7000-8000-00000000d102",
          type: "koshFileAttachment",
          props: {
            attachmentId,
            byteLength: 2_048,
            caption: "",
            displayFilename: "vectors.csv",
            mediaType: "text/csv",
          },
        },
      ],
    }),
    sources: [],
  });
  await page.keyboard.press("Meta+k");
  await page.getByRole("combobox", { name: "Search notes" }).fill("vectors.csv");
  await page.getByRole("option", { name: /Vector chapter/u }).click();

  await expect(page.locator('[data-kosh-search-hit="true"]')).toContainText("vectors.csv");
  await expect(page).toHaveURL(
    `http://127.0.0.1:1422/#/notes/${note.id}?blockId=019f547b-6200-7000-8000-00000000d102`,
  );
  await page.waitForTimeout(1_500);
  await expect(page.locator('[data-kosh-search-hit="true"]')).toHaveCount(0);
});

test("StrictMode keeps semantic polling bounded to the open overlay", async ({ page }) => {
  await page.addInitScript(() => {
    const nativeSetTimeout = window.setTimeout.bind(window);
    const nativeClearTimeout = window.clearTimeout.bind(window);
    const semanticTimers = new Map<number, TimerHandler>();
    let nextTimerId = 900_000;

    window.setTimeout = ((handler: TimerHandler, timeout?: number, ...arguments_: unknown[]) => {
      if (timeout === 2_000) {
        const timerId = nextTimerId++;
        semanticTimers.set(timerId, () => {
          if (typeof handler === "function") handler(...arguments_);
          else Function(handler)();
        });
        return timerId;
      }
      return nativeSetTimeout(handler, timeout, ...arguments_);
    }) as typeof window.setTimeout;
    window.clearTimeout = ((timerId: number | undefined) => {
      if (timerId !== undefined && semanticTimers.delete(timerId)) return;
      nativeClearTimeout(timerId);
    }) as typeof window.clearTimeout;
    Reflect.set(window, "__KOSH_RUN_SEMANTIC_TIMER__", () => {
      const first = semanticTimers.entries().next().value as [number, TimerHandler] | undefined;
      if (!first) throw new Error("semantic timer was not scheduled");
      semanticTimers.delete(first[0]);
      if (typeof first[1] === "function") first[1]();
    });
    Object.defineProperty(window, "__KOSH_SEMANTIC_TIMER_COUNT__", {
      configurable: true,
      get: () => semanticTimers.size,
    });
  });
  await page.goto("/#/");
  expect(await page.evaluate(() => Reflect.get(window, "__KOSH_SEMANTIC_TIMER_COUNT__"))).toBe(0);
  await page.keyboard.press("Meta+k");
  await expect
    .poll(() => page.evaluate(() => Reflect.get(window, "__KOSH_SEMANTIC_TIMER_COUNT__")))
    .toBe(1);

  await page.evaluate(() => Reflect.get(window, "__KOSH_RUN_SEMANTIC_TIMER__")());
  await expect
    .poll(() => page.evaluate(() => Reflect.get(window, "__KOSH_SEMANTIC_TIMER_COUNT__")))
    .toBe(1);
  await page.keyboard.press("Escape");
  await expect
    .poll(() => page.evaluate(() => Reflect.get(window, "__KOSH_SEMANTIC_TIMER_COUNT__")))
    .toBe(0);
});

async function seedTidbit(page: Page, input: FakeNoteInput): Promise<TidbitRecord> {
  return page.evaluate(async (draft) => {
    const backend = window.__KOSH_FAKE_BACKEND__;
    if (!backend) throw new Error("fake backend is unavailable");
    return backend.seedNote(draft);
  }, input);
}

async function searchStorageKeys(page: Page): Promise<string[]> {
  return page.evaluate(() =>
    Object.keys(localStorage).filter((key) => /search|query|history/iu.test(key)),
  );
}

function documentBlockIds(note: TidbitRecord): string[] {
  return (JSON.parse(note.documentJson) as { blocks: Array<{ id: string }> }).blocks.map(
    ({ id }) => id,
  );
}

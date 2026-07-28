import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";
import type { TidbitDraft, TidbitRecord } from "../../src/backend/contracts";

test("search-as-you-type renders cited passages, exact mode, and keyboard history", async ({
  context,
  page,
}) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.goto("/#/");
  const search = page.getByRole("searchbox", { name: "Search tidbits" });
  await expect(search).toBeFocused();
  await seedTidbit(page, {
    title: "Tomato technique",
    bodyMarkdown: "Slow simmering preserves a bright tomato sauce.",
    sources: [{ label: "Cookbook", url: "https://www.example.com/tomato" }],
  });
  await seedTidbit(page, {
    title: "Second tomato note",
    bodyMarkdown: "Roast tomato halves before blending the sauce.",
    sources: [{ label: "Kitchen log", url: "https://notes.example.org/roasting" }],
  });

  await search.fill("tomato");
  const options = page.getByRole("option");
  await expect(options).toHaveCount(2);
  await expect(page.getByText("Lexical matches", { exact: true })).toBeVisible();
  await expect(
    page.getByText("Semantic search is off · lexical search still works", { exact: true }),
  ).toBeVisible();
  await expect(options.first().locator("mark").first()).toHaveText(/tomato/iu);
  await expect(options.first()).toContainText("example.org");
  await page.getByRole("button", { name: "Enable semantic" }).click();
  await expect(page.getByText("Semantic ready", { exact: true })).toBeVisible();
  await expect(
    page.getByText("Semantic index ready · this result used lexical retrieval", { exact: true }),
  ).toBeVisible();

  await search.press("ArrowDown");
  await expect(options.first()).toBeFocused();
  await expect(page.locator("#search-citation-detail")).toContainText("Second tomato note");
  await options.first().press("ArrowDown");
  await expect(options.nth(1)).toBeFocused();
  await options.nth(1).press("Enter");
  await expect(page.locator("#search-citation-detail")).toBeFocused();
  await expect(page.locator("#search-citation-detail")).toContainText("Cookbook · example.com");

  await page.getByRole("button", { name: "Copy citation" }).click();
  await expect(page.getByText("Citation copied", { exact: true })).toBeVisible();

  await page.goBack();
  await expect(page.locator("#search-citation-detail")).toContainText("Second tomato note");
  await expect(search).toHaveValue("tomato");
  await page.goForward();
  await expect(page.locator("#search-citation-detail")).toContainText("Tomato technique");

  await page.getByRole("checkbox", { name: "Exact" }).check();
  await expect(page.getByText("Exact lexical matches", { exact: true })).toBeVisible();
  await expect(
    page.getByText("Exact mode uses lexical retrieval by design", { exact: true }),
  ).toBeVisible();
  await expect(page.locator(".search-command__research")).toHaveAttribute("href", "/#/research");
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
});

test("superseded and failed searches cannot replace a newer result", async ({ page }) => {
  await page.goto("/#/");
  await seedTidbit(page, {
    title: "Slow response",
    bodyMarkdown: "Only the slow query should find this passage.",
    sources: [],
  });
  await seedTidbit(page, {
    title: "Fast response",
    bodyMarkdown: "Only the fast query should find this passage.",
    sources: [],
  });
  await page.evaluate(() => {
    const backend = window.__KOSH_FAKE_BACKEND__;
    if (!backend) throw new Error("fake backend is unavailable");
    const search = backend.searchPassages.bind(backend);
    let failedOnce = false;
    backend.searchPassages = async (input) => {
      if (input.query === "slow") {
        await new Promise((resolve) => window.setTimeout(resolve, 500));
      }
      if (input.query === "explode" && !failedOnce) {
        failedOnce = true;
        throw new Error("controlled search failure");
      }
      return search(input);
    };
  });

  const input = page.getByRole("searchbox", { name: "Search tidbits" });
  await input.fill("slow");
  await page.waitForTimeout(240);
  await input.fill("fast");
  await expect(page.getByRole("option", { name: /Fast response/u })).toBeVisible();
  await page.waitForTimeout(550);
  await expect(page.getByRole("option", { name: /Fast response/u })).toBeVisible();
  await expect(page.getByRole("option", { name: /Slow response/u })).toHaveCount(0);

  await input.fill("explode");
  await expect(page.getByRole("alert")).toContainText("controlled search failure");
  await page.getByRole("button", { name: "Try again" }).click();
  await expect(page.getByRole("heading", { name: "No supporting passages" })).toBeVisible();
  await input.fill("fast");
  await expect(page.getByRole("option", { name: /Fast response/u })).toBeVisible();
});

test("a citation edited after search opens as historical and focuses its exact passage", async ({
  page,
}) => {
  await page.goto("/#/");
  const created = await seedTidbit(page, {
    title: "Revision evidence",
    bodyMarkdown: "The original immutable passage mentions cobalt.",
    sources: [{ label: "Lab notebook", url: "https://example.com/lab" }],
  });
  const search = page.getByRole("searchbox", { name: "Search tidbits" });
  await search.fill("cobalt");
  const result = page.getByRole("option", { name: /Revision evidence/u });
  await expect(result).toBeVisible();

  await page.evaluate(async (tidbitId) => {
    const backend = window.__KOSH_FAKE_BACKEND__;
    if (!backend) throw new Error("fake backend is unavailable");
    const current = await backend.loadTidbit(tidbitId);
    await backend.editTidbit({
      id: current.id,
      expectedRevisionId: current.currentRevisionId,
      title: current.title,
      bodyMarkdown: "The current passage now mentions indigo.",
      sources: current.sources,
    });
  }, created.id);

  await result.click();
  await expect(page.getByText("Historical passage", { exact: true })).toBeVisible();
  await expect(page.locator("#search-citation-detail")).toContainText(
    "The original immutable passage mentions cobalt.",
  );
  await page.getByRole("link", { name: "Open tidbit at passage" }).click();
  const citedPassage = page.getByRole("region", { name: "Revision evidence" });
  await expect(citedPassage).toBeFocused();
  await expect(citedPassage).toContainText("The original immutable passage mentions cobalt.");
  await expect(page.getByText("The current passage now mentions indigo.")).toBeVisible();
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
});

async function seedTidbit(page: Page, input: TidbitDraft): Promise<TidbitRecord> {
  return page.evaluate(async (draft) => {
    const backend = window.__KOSH_FAKE_BACKEND__;
    if (!backend) throw new Error("fake backend is unavailable");
    return backend.createTidbit(draft);
  }, input);
}

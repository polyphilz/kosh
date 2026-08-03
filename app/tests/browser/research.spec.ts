import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "./fixtures";

test("research completes with trusted citations, durable history, and save-as-tidbit", async ({
  page,
}) => {
  await page.goto("/#/research");
  await page.evaluate(async () => {
    const backend = window.__KOSH_FAKE_BACKEND__;
    if (!backend) throw new Error("fake backend is unavailable");
    await backend.createTidbit({
      title: "Browser evidence",
      bodyMarkdown: "An exact browser-test passage.",
      sources: [{ label: "Local notebook", url: "https://example.com/local" }],
    });
  });

  await page
    .getByRole("textbox", { name: "What should Kosh investigate?" })
    .fill("Synthesize my local evidence");
  await page.getByRole("button", { name: "Research" }).click();
  await expect(page.getByText(/Kosh found a durable answer/u)).toBeVisible();
  await page.getByRole("button", { name: "Open citation 1" }).click();
  await expect(page.locator("#search-citation-detail")).toContainText("Browser evidence");
  await expect(page.locator("#search-citation-detail")).toContainText(
    "An exact browser-test passage.",
  );

  await page.getByRole("button", { name: "Save answer as tidbit" }).click();
  await expect(page.getByRole("link", { name: "Open saved tidbit" })).toBeVisible();
  const navigation = page.getByRole("navigation", { name: "Primary" });
  await navigation.getByRole("button", { name: "Search", exact: true }).click();
  await expect(page.getByRole("dialog", { name: "Search notes" })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(navigation.getByText("Research", { exact: true })).toHaveCount(0);
  await page.goto("/#/research");
  await expect(page.getByText(/Kosh found a durable answer/u)).toBeVisible();
  await expect(page.getByRole("link", { name: "Open saved tidbit" })).toBeVisible();
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
});

test("research cancellation and failure remain recoverable", async ({ page }) => {
  await page.goto("/#/research");
  const query = page.getByRole("textbox", { name: "What should Kosh investigate?" });
  await query.fill("[slow] stop this run");
  await page.getByRole("button", { name: "Research" }).click();
  await page.getByRole("button", { name: "Cancel" }).click();
  await expect(page.getByRole("heading", { name: "Research canceled" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Run again" })).toBeVisible();

  await query.fill("[fail] controlled failure");
  await page.getByRole("button", { name: "Research" }).click();
  await expect(page.getByText("Fixture research failed safely.")).toBeVisible();
  await expect(page.getByRole("button", { name: "Run again" })).toBeVisible();
});

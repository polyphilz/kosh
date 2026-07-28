import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";

test("capture, edit, and delete share the complete tidbit workflow", async ({ page }) => {
  await page.goto("/#/add");
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
  await page.getByRole("textbox", { name: /^Title/u }).fill("Browser tidbit");
  await page.getByRole("textbox", { name: "Tidbit" }).fill("Knowledge with **evidence**.");
  await page.getByRole("button", { name: "Add source" }).click();
  await page.getByRole("textbox", { name: "Source 1 label" }).fill("Docs");
  await page
    .getByRole("textbox", { name: "Source 1 URL" })
    .fill("https://example.com/docs#section");
  await page.getByRole("button", { name: "Save tidbit" }).click();

  await expect(page.getByRole("heading", { name: "Browser tidbit" })).toBeVisible();
  await expect(page.getByText("Docs", { exact: true })).toBeVisible();
  await expect(page.getByText("https://example.com/docs", { exact: true })).toBeVisible();
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);

  await page.getByRole("button", { name: "Edit" }).click();
  await page.getByRole("textbox", { name: /^Title/u }).fill("Edited browser tidbit");
  await page.getByRole("textbox", { name: "Tidbit" }).pressSequentially(" More context.");
  await page.getByRole("button", { name: "Save changes" }).click();

  await expect(page.getByRole("heading", { name: "Edited browser tidbit" })).toBeVisible();
  await expect(page.getByText(/More context/u)).toBeVisible();

  await page.getByRole("button", { name: "Delete" }).click();
  await expect(page.getByRole("dialog", { name: "Delete this tidbit?" })).toBeVisible();
  await page.getByRole("button", { name: "Delete tidbit" }).click();
  await expect(page.getByRole("heading", { name: "Search" })).toBeVisible();
});

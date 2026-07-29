import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "@playwright/test";

test("library, history, trash, and restore form one operable lifecycle", async ({ page }) => {
  await createTidbit(page, "First library note", "Original local evidence.");
  await page.getByRole("button", { name: "Edit" }).click();
  await page.getByRole("textbox", { name: /^Title/u }).fill("Revised library note");
  await page.getByRole("textbox", { name: "Tidbit" }).pressSequentially(" Updated.");
  await page.getByRole("button", { name: "Save changes" }).click();

  await expect(page.getByRole("heading", { name: "Revised library note" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Revision history" })).toBeVisible();
  await page.getByRole("button", { name: /Revision 1.*First library note/u }).click();
  await expect(page.getByRole("heading", { name: "First library note" })).toBeVisible();
  await page.getByRole("button", { name: "Return to current" }).click();
  await expect(page.getByRole("heading", { name: "Revised library note" })).toBeVisible();

  await page.getByRole("button", { name: "Delete" }).click();
  await page.getByRole("button", { name: "Move to Trash" }).click();
  await expect(page.getByRole("link", { name: /Revised library note/u })).toBeVisible();
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);

  await page.getByRole("link", { name: /Revised library note/u }).click();
  await expect(page.getByText("This tidbit is in Trash.")).toBeVisible();
  await expect(page.getByRole("button", { name: "Delete permanently" })).toBeDisabled();
  await page.getByRole("button", { name: "Restore" }).click();
  await expect(page.getByText("Tidbit restored")).toBeVisible();
  await page.getByRole("link", { name: "← Back to library" }).click();
  await expect(page.getByRole("heading", { name: "Trash is empty" })).toBeVisible();
});

test("library surface stays visually stable", async ({ page }) => {
  await createTidbit(page, "Alpha note", "A compact thought.");
  await page.getByRole("link", { name: "Add" }).click();
  await page.getByRole("textbox", { name: /^Title/u }).fill("Beta chapter notes");
  await page
    .getByRole("textbox", { name: "Tidbit" })
    .fill("# Chapter 2\n\nA longer observation with `code` and $x^2$.");
  await page.getByRole("button", { name: "Save tidbit" }).click();
  await page.getByRole("link", { name: "Library", exact: true }).click();

  await expect(page.getByRole("heading", { name: "Library" })).toBeVisible();
  await expect(page).toHaveScreenshot("library-recent.png", {
    animations: "disabled",
    fullPage: true,
    mask: [page.locator(".library-list time")],
    maskColor: "#d8d2ca",
  });
});

async function createTidbit(page: import("@playwright/test").Page, title: string, body: string) {
  await page.goto("/#/add");
  await page.getByRole("textbox", { name: /^Title/u }).fill(title);
  await page.getByRole("textbox", { name: "Tidbit" }).fill(body);
  await page.getByRole("button", { name: "Save tidbit" }).click();
}

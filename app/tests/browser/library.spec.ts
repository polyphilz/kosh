import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "./fixtures";

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

async function createTidbit(page: Page, title: string, body: string) {
  await page.goto("/#/add");
  await page.getByRole("textbox", { name: /^Title/u }).fill(title);
  await page.getByRole("textbox", { name: "Tidbit" }).fill(body);
  await page.getByRole("button", { name: "Save tidbit" }).click();
}

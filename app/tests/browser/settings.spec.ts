import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "./fixtures";

test("settings exposes local diagnostics and guarded maintenance", async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.goto("/#/settings");

  await expect(page.getByRole("heading", { name: "Offsite recovery" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Data & diagnostics" })).toBeVisible();
  await expect(
    page.getByText("This is backup, not multi-device sync.", { exact: false }),
  ).toBeVisible();
  await page.getByText("Local paths").click();
  await expect(page.getByText("/tmp/kosh-browser-fixture/kosh.sqlite3")).toBeVisible();

  await page.getByRole("button", { name: "Check integrity" }).click();
  const dialog = page.getByRole("dialog", { name: "Check local data?" });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByText("Authored data will not change.", { exact: false })).toBeVisible();
  await dialog.getByRole("button", { name: "Run integrity check" }).click();
  await expect(
    page.getByText("Both databases and all referenced media passed integrity checks."),
  ).toBeVisible();

  const results = await new AxeBuilder({ page }).analyze();
  expect(results.violations).toEqual([]);
  await page.getByRole("heading", { name: "Maintenance" }).scrollIntoViewIfNeeded();
  await expect(page.getByRole("heading", { name: "Maintenance" })).toBeVisible();
});

test("settings configures, enables, backs up, and drills recovery without exposing secrets", async ({
  page,
}) => {
  const accessKeyId = "fedcba9876543210fedcba9876543210";
  const secretAccessKey = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

  await page.goto("/#/settings");
  const recovery = page.getByRole("region", { name: "Offsite recovery" });
  await expect(recovery.getByRole("button", { name: "Save target off" })).toBeVisible();
  await recovery.getByLabel("Cloudflare account ID").fill("0123456789abcdef0123456789abcdef");
  await recovery.getByLabel("R2 bucket").fill("kosh-local");
  await recovery.getByLabel("R2 access key ID").fill(accessKeyId);
  await recovery.getByLabel("R2 secret access key").fill(secretAccessKey);

  await recovery.getByRole("button", { name: "Test connection" }).click();
  await expect(
    recovery.getByText("Connection verified. Kosh wrote, read, listed", { exact: false }),
  ).toBeVisible();
  await recovery.getByRole("button", { name: "Save target off" }).click();

  await expect(recovery.getByText("kosh-local")).toBeVisible();
  await expect(recovery.getByText("Stored", { exact: true })).toBeVisible();
  const renderedInputValues = await recovery
    .locator("input")
    .evaluateAll((inputs) => inputs.map((input) => (input as HTMLInputElement).value));
  expect(renderedInputValues).not.toContain(accessKeyId);
  expect(renderedInputValues).not.toContain(secretAccessKey);

  const enabled = recovery.getByRole("switch", { name: "Back up this library" });
  await enabled.click();
  await expect(enabled).toBeChecked();
  await recovery.getByRole("button", { name: "Back up now" }).click();
  await expect(recovery.getByText("A complete recovery point was published.")).toBeVisible();
  await recovery.getByRole("button", { name: "Find recovery points" }).click();
  await expect(recovery.getByText("Found 1 complete recovery point.")).toBeVisible();
  await recovery.getByRole("button", { name: "Preview restore" }).click();
  await expect(recovery.getByText("Verified restore preview")).toBeVisible();
  await recovery.getByRole("button", { name: "Run recovery drill" }).click();
  await expect(
    recovery.getByText("Your live library was not changed.", { exact: false }),
  ).toBeVisible();

  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
});

import AxeBuilder from "@axe-core/playwright";
import { expect, test } from "./fixtures";

test("note-first capture preserves image, PDF, file, pasted-link, and citation surfaces", async ({
  page,
}) => {
  await page.route("kosh-media://**", async (route) => {
    await route.fulfill({
      body: Buffer.from(
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=",
        "base64",
      ),
      contentType: "image/png",
    });
  });
  await page.goto("/#/");
  await page.evaluate(() => {
    const backend = window.__KOSH_FAKE_BACKEND__;
    if (!backend) throw new Error("fake backend is unavailable");

    backend.selectImage = async () => "note-image-selection";
    backend.ingestSelectedImage = async () => ({
      id: "019f547b-6200-7000-8000-00000000c001",
      ingestLeaseId: "019f547b-6200-7000-8000-00000000c002",
      displayFilename: "vector-board.png",
      mediaType: "image/png",
      byteLength: 1_024,
      kind: "IMAGE",
      naturalWidth: 1_200,
      naturalHeight: 800,
      ocrStatus: "READY",
      ocrError: null,
    });
    backend.imageStatus = async (attachmentId) => ({
      attachmentId,
      naturalWidth: 1_200,
      naturalHeight: 800,
      ocrStatus: "READY",
      ocrError: null,
      nextAttemptAtMs: null,
    });

    backend.selectPdf = async () => "note-pdf-selection";
    backend.ingestSelectedPdf = async () => ({
      id: "019f547b-6200-7000-8000-00000000c011",
      ingestLeaseId: "019f547b-6200-7000-8000-00000000c012",
      displayFilename: "vector-chapter.pdf",
      mediaType: "application/pdf",
      byteLength: 2_048,
      kind: "PDF",
      pageCount: 3,
      extractionStatus: "READY",
      extractionError: null,
    });
    backend.pdfStatus = async (attachmentId) => ({
      attachmentId,
      displayFilename: "vector-chapter.pdf",
      pageCount: 3,
      extractedPageCount: 3,
      unavailablePageCount: 0,
      extractionStatus: "READY",
      extractionError: null,
      nextAttemptAtMs: null,
    });

    backend.selectAttachment = async () => "note-file-selection";
    backend.ingestSelectedAttachment = async () => ({
      recordKind: "GENERIC",
      record: {
        id: "019f547b-6200-7000-8000-00000000c021",
        ingestLeaseId: "019f547b-6200-7000-8000-00000000c022",
        displayFilename: "vector-scraps.md",
        mediaType: "text/markdown",
        byteLength: 512,
        kind: "TEXT",
        extractionStatus: "READY",
        extractionError: null,
        extractedLineCount: 8,
      },
    });
    backend.attachmentStatus = async (attachmentId) => ({
      attachmentId,
      displayFilename: "vector-scraps.md",
      mediaType: "text/markdown",
      byteLength: 512,
      kind: "TEXT",
      extractionStatus: "READY",
      extractionError: null,
      extractedLineCount: 8,
    });
  });

  const editor = page.getByRole("textbox", { name: "Note" });
  await editor.fill("Vector note");
  await chooseSlashItem(page, "Image");
  await expect(page.locator("[data-kosh-image='true']")).toBeVisible();
  await page.getByRole("textbox", { name: "Alt text" }).fill("Vector board");
  await chooseSlashItem(page, "PDF");
  await expect(page.locator("[data-kosh-pdf='true']")).toContainText("vector-chapter.pdf");
  await chooseSlashItem(page, "File");
  await expect(page.locator("[data-kosh-file='true']")).toContainText("vector-scraps.md");
  await editor.focus();
  await editor.press("Control+End");
  await editor.press("Enter");
  await editor.pressSequentially(
    "The exact note passage remembers contiguous arrays. https://example.com/numpy-vectors",
  );

  await expect(page).toHaveURL(/\/#\/notes\/[0-9a-f-]{36}$/u, { timeout: 5_000 });
  await expect(page.getByRole("img", { name: "Vector board" })).toBeVisible();
  await expect(page.locator("[data-kosh-pdf='true']")).toContainText("vector-chapter.pdf");
  await expect(page.locator("[data-kosh-file='true']")).toContainText("vector-scraps.md");

  await page.getByRole("button", { name: "Search", exact: true }).click();
  await page.getByRole("combobox", { name: "Search notes" }).fill("contiguous arrays");
  const citation = page.getByRole("option", { name: /Vector note/u });
  await expect(citation).toContainText("The exact note passage remembers contiguous arrays.");
  await expect(citation).toContainText("numpy-vectors");
  await citation.click();
  await expect(page.locator('[data-kosh-search-hit="true"]')).toContainText("contiguous arrays");
  await expect(page.getByLabel("Search result location")).toHaveCount(0);
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
});

async function chooseSlashItem(page: import("@playwright/test").Page, name: string) {
  const editor = page.getByRole("textbox", { name: "Note" });
  await editor.focus();
  await editor.press("Control+End");
  await editor.press("Enter");
  await editor.pressSequentially("/");
  await page.getByRole("option", { name, exact: true }).click();
}

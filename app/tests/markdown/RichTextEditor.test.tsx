import { EditorView as CodeMirrorView } from "@codemirror/view";
import { act, fireEvent, render, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StrictMode, createRef } from "react";
import { NodeSelection, TextSelection } from "prosemirror-state";
import { expect, it, vi } from "vitest";
import type { GenericAttachmentRecord, ImageRecord, PdfRecord } from "../../src/backend/contracts";
import { koshEditorSchema } from "../../src/markdown/editorSchema";
import { richTextEditorViewFromDOM } from "../../src/markdown/editorViewRegistry";
import { statusPollDelay } from "../../src/markdown/ImageNodeView";
import { parseKoshMarkdown, serializeKoshMarkdown } from "../../src/markdown/markdownConversion";
import { RichTextEditor, type RichTextEditorHandle } from "../../src/markdown/RichTextEditor";

it("parses every supported construct and serializes one stable spelling", () => {
  const source = [
    "# Heading",
    "",
    "**bold** and *italic* with ~~strike~~, `code`, [docs](https://example.com), and $x^2$.",
    "",
    "> quote",
    "",
    "- first",
    "- [x] done",
    "",
    "```TS",
    "const answer = 42",
    "```",
    "",
    "| A | B |",
    "| - | - |",
    "| 1 | 2 |",
    "",
    "---",
    "",
    "$$",
    "E = mc^2",
    "$$",
  ].join("\n");

  const canonical = serializeKoshMarkdown(parseKoshMarkdown(source, koshEditorSchema));
  expect(serializeKoshMarkdown(parseKoshMarkdown(canonical, koshEditorSchema))).toBe(canonical);
  expect(canonical).toContain("```typescript");
  expect(canonical).toContain("- [x] done");
  expect(canonical).toContain("| A | B |");
  expect(canonical).toContain("$$\nE = mc^2\n$$");
});

it("canonicalizes consumed link definitions without dropping unused definitions", () => {
  const source = [
    "Read [the docs][docs].",
    "",
    "[docs]: https://example.com/guide",
    "[unused]: https://unused.example/reference",
  ].join("\n");

  const canonical = serializeKoshMarkdown(parseKoshMarkdown(source, koshEditorSchema));

  expect(canonical).toContain("[the docs](https://example.com/guide)");
  expect(canonical).not.toContain("[docs]:");
  expect(canonical).toContain("unused");
  expect(canonical).toContain("https://unused.example/reference");
  expect(serializeKoshMarkdown(parseKoshMarkdown(canonical, koshEditorSchema))).toBe(canonical);
});

it("resolves definitions nested inside Markdown containers", () => {
  const source = ["> Read [nested docs][docs].", ">", "> [docs]: https://example.com/nested"].join(
    "\n",
  );

  const canonical = serializeKoshMarkdown(parseKoshMarkdown(source, koshEditorSchema));

  expect(canonical).toBe("> Read [nested docs](https://example.com/nested).");
  expect(serializeKoshMarkdown(parseKoshMarkdown(canonical, koshEditorSchema))).toBe(canonical);
});

it("starts with the controlled value and reports canonical document changes", () => {
  const onChange = vi.fn();
  const { getByRole } = render(
    <RichTextEditor ariaLabel="Body" onChange={onChange} value="initial" />,
  );
  const textbox = getByRole("textbox", { name: "Body" });
  expectWritingAssistanceDisabled(textbox);
  const view = editorView(textbox);

  act(() => {
    view.dispatch(view.state.tr.insertText(" value", 8));
  });

  expect(onChange).toHaveBeenCalledOnce();
  expect(onChange).toHaveBeenCalledWith("initial value");
});

it("replaces an external value without recreating the editor or echoing onChange", () => {
  const onChange = vi.fn();
  const { getByRole, rerender } = render(
    <RichTextEditor ariaLabel="Body" onChange={onChange} value="one" />,
  );
  const first = editorView(getByRole("textbox", { name: "Body" }));

  rerender(<RichTextEditor ariaLabel="Body" onChange={onChange} value="**replacement**" />);

  const current = editorView(getByRole("textbox", { name: "Body" }));
  expect(current).toBe(first);
  expect(current.state.doc.textContent).toBe("replacement");
  expect(current.state.doc.rangeHasMark(1, 12, koshEditorSchema.marks.strong!)).toBe(true);
  expect(onChange).not.toHaveBeenCalled();
});

it("resets undo history when replacing the controlled document", () => {
  const onChange = vi.fn();
  const { getByRole, rerender } = render(
    <RichTextEditor ariaLabel="Body" onChange={onChange} value="first tidbit" />,
  );
  const textbox = getByRole("textbox", { name: "Body" });
  const view = editorView(textbox);
  act(() => {
    view.dispatch(view.state.tr.insertText(" edited", view.state.doc.content.size - 1));
  });

  rerender(<RichTextEditor ariaLabel="Body" onChange={onChange} value="second tidbit" />);
  fireEvent.keyDown(textbox, { key: "z", metaKey: true });

  expect(serializeKoshMarkdown(view.state.doc)).toBe("second tidbit");
});

it("updates the mounted textbox label and placeholder", () => {
  const { getByRole, rerender } = render(
    <RichTextEditor
      ariaLabel="Original body"
      onChange={() => undefined}
      placeholder="Original hint"
      value=""
    />,
  );
  const textbox = getByRole("textbox", { name: "Original body" });
  expect(textbox).toHaveAttribute("data-placeholder", "Original hint");

  rerender(
    <RichTextEditor
      ariaLabel="Updated body"
      onChange={() => undefined}
      placeholder="Updated hint"
      value=""
    />,
  );

  expect(getByRole("textbox", { name: "Updated body" })).toBe(textbox);
  expect(textbox).toHaveAttribute("data-placeholder", "Updated hint");
});

it("supports imperative focus and disabled state without replacing the view", () => {
  const ref = createRef<RichTextEditorHandle>();
  const { getByRole, rerender } = render(
    <RichTextEditor ariaLabel="Body" onChange={() => undefined} ref={ref} value="value" />,
  );
  const textbox = getByRole("textbox", { name: "Body" });
  const view = editorView(textbox);
  act(() => ref.current?.focus());
  expect(document.activeElement).toBe(textbox);

  rerender(
    <RichTextEditor ariaLabel="Body" disabled onChange={() => undefined} ref={ref} value="value" />,
  );

  expect(editorView(getByRole("textbox", { name: "Body" }))).toBe(view);
  expect(textbox).toHaveAttribute("contenteditable", "false");
});

it("applies toolbar and keyboard formatting while retaining editor focus", () => {
  const { getByRole, textbox, view } = controlledEditor("word");
  act(() => {
    view.dispatch(view.state.tr.setSelection(TextSelection.create(view.state.doc, 1, 5)));
    view.focus();
  });

  fireEvent.click(getByRole("button", { name: "Bold" }));
  fireEvent.keyDown(textbox, { key: "i", metaKey: true });

  const canonical = serializeKoshMarkdown(view.state.doc);
  expect(canonical).toContain("***word***");
  expect(document.activeElement).toBe(textbox);
});

it("creates inline code without retaining typed delimiters", () => {
  const { view } = controlledEditor("");

  act(() => typeWithInputRules(view, "`value`"));

  expect(serializeKoshMarkdown(view.state.doc)).toBe("`value`");
  expect(view.state.doc.textContent).toBe("value");
});

it("activates toolbar buttons through the keyboard click path", async () => {
  const user = userEvent.setup();
  const { getByRole, view } = controlledEditor("word");
  act(() => {
    view.dispatch(view.state.tr.setSelection(TextSelection.create(view.state.doc, 1, 5)));
  });

  getByRole("button", { name: "Bold" }).focus();
  await user.keyboard("{Enter}");

  expect(serializeKoshMarkdown(view.state.doc)).toBe("**word**");
});

it("validates links and serializes only absolute HTTP destinations", () => {
  const { getByRole, view } = controlledEditor("word");
  act(() => {
    view.dispatch(view.state.tr.setSelection(TextSelection.create(view.state.doc, 1, 5)));
  });
  fireEvent.click(getByRole("button", { name: "Link" }));
  const input = getByRole("textbox", { name: "Link URL" });
  fireEvent.change(input, { target: { value: "javascript:alert(1)" } });
  fireEvent.click(getByRole("button", { name: "Apply" }));
  expect(getByRole("alert")).toHaveTextContent("http:// or https://");

  fireEvent.change(input, { target: { value: "https://example.com" } });
  fireEvent.click(getByRole("button", { name: "Apply" }));
  expect(serializeKoshMarkdown(view.state.doc)).toBe("[word](https://example.com/)");
});

it("treats clearing a link at an unlinked cursor as a safe no-op", () => {
  const { getByRole, view } = controlledEditor("word");
  fireEvent.click(getByRole("button", { name: "Link" }));
  fireEvent.change(getByRole("textbox", { name: "Link URL" }), {
    target: { value: "" },
  });
  fireEvent.click(getByRole("button", { name: "Apply" }));

  expect(serializeKoshMarkdown(view.state.doc)).toBe("word");
  expect(getByRole("textbox", { name: "Editor" })).toHaveFocus();
});

it("updates the complete link mark when editing at a collapsed cursor", () => {
  const { getByRole, view } = controlledEditor("[word](https://old.example/)");
  act(() => {
    view.dispatch(view.state.tr.setSelection(TextSelection.create(view.state.doc, 3)));
  });
  fireEvent.click(getByRole("button", { name: "Link" }));
  const input = getByRole("textbox", { name: "Link URL" });
  expect(input).toHaveValue("https://old.example/");
  fireEvent.change(input, { target: { value: "https://new.example/path" } });
  fireEvent.click(getByRole("button", { name: "Apply" }));

  expect(serializeKoshMarkdown(view.state.doc)).toBe("[word](https://new.example/path)");
});

it("inserts and renders inline math from the editor-owned dialog", () => {
  const { container, getByRole, view } = controlledEditor("formula:");
  act(() => {
    view.dispatch(view.state.tr.insertText(" ", view.state.doc.content.size - 1));
    view.dispatch(view.state.tr.setSelection(TextSelection.atEnd(view.state.doc)));
  });

  fireEvent.click(getByRole("button", { name: "Inline math" }));
  const formula = getByRole("textbox", { name: "Formula" });
  fireEvent.change(formula, { target: { value: "E = mc^2" } });
  expect(
    getByRole("dialog", { name: "Inline math editor" }).querySelector(".katex"),
  ).not.toBeNull();
  fireEvent.click(getByRole("button", { name: "Apply" }));

  expect(serializeKoshMarkdown(view.state.doc)).toBe("formula: $E = mc^2$");
  expect(container.querySelector(".kosh-math-inline .katex")).not.toBeNull();
});

it("closes math editors across document mutations and controlled replacements", () => {
  const result = render(
    <RichTextEditor ariaLabel="Body" onChange={() => undefined} value="Before $x$ after." />,
  );
  const view = editorView(result.getByRole("textbox", { name: "Body" }));

  fireEvent.doubleClick(result.container.querySelector(".kosh-math-inline")!);
  expect(result.getByRole("dialog", { name: "Inline math editor" })).toBeInTheDocument();
  act(() => {
    view.dispatch(view.state.tr.insertText("Prefix ", 1));
  });
  expect(result.queryByRole("dialog", { name: "Inline math editor" })).toBeNull();
  expect(serializeKoshMarkdown(view.state.doc)).toContain("$x$");

  fireEvent.doubleClick(result.container.querySelector(".kosh-math-inline")!);
  expect(result.getByRole("dialog", { name: "Inline math editor" })).toBeInTheDocument();
  result.rerender(
    <RichTextEditor ariaLabel="Body" onChange={() => undefined} value="Replacement $y$." />,
  );

  expect(result.queryByRole("dialog", { name: "Inline math editor" })).toBeNull();
  expect(serializeKoshMarkdown(view.state.doc)).toBe("Replacement $y$.");
});

it("does not open or mutate math nodes while disabled", () => {
  const onChange = vi.fn();
  const { container, queryByRole } = render(
    <RichTextEditor ariaLabel="Body" disabled onChange={onChange} value="Inline $x^2$." />,
  );
  const math = container.querySelector(".kosh-math-inline");
  expect(math).not.toBeNull();
  fireEvent.doubleClick(math!);
  fireEvent.keyDown(math!, { key: "Enter" });

  expect(queryByRole("dialog", { name: "Inline math editor" })).toBeNull();
  expect(onChange).not.toHaveBeenCalled();
});

it("lazy-loads CodeMirror, synchronizes code, and changes fence language", async () => {
  const { container, getByRole, view } = controlledEditor("```typescript\nconst answer = 42\n```");

  const codeView = await embeddedCodeView(container);
  act(() => {
    codeView.focus();
    codeView.dispatch({
      changes: {
        from: codeView.state.doc.length,
        insert: "\nconsole.log(answer)",
      },
    });
  });
  expect(serializeKoshMarkdown(view.state.doc)).toContain("console.log(answer)");

  fireEvent.change(getByRole("combobox", { name: "Code language" }), {
    target: { value: "python" },
  });
  expect(serializeKoshMarkdown(view.state.doc)).toContain("```python");
});

it("propagates disabled changes into an existing CodeMirror node view", async () => {
  const onChange = vi.fn();
  const result = render(
    <RichTextEditor
      ariaLabel="Body"
      onChange={onChange}
      value={"```typescript\nconst answer = 42\n```"}
    />,
  );
  const view = editorView(result.getByRole("textbox", { name: "Body" }));
  const codeView = await embeddedCodeView(result.container);
  act(() => {
    codeView.focus();
    codeView.dispatch({
      changes: { from: codeView.state.doc.length, insert: "\ncommitted edit" },
    });
  });
  expect(serializeKoshMarkdown(view.state.doc)).toContain("committed edit");
  onChange.mockClear();

  result.rerender(
    <RichTextEditor
      ariaLabel="Body"
      disabled
      onChange={onChange}
      value={"```typescript\nconst answer = 42\n```"}
    />,
  );

  expect(codeView.state.readOnly).toBe(true);
  expect(codeView.contentDOM).toHaveAttribute("contenteditable", "false");
  const childCount = view.state.doc.childCount;
  expect(fireEvent.keyDown(codeView.contentDOM, { key: "z", metaKey: true })).toBe(false);
  expect(fireEvent.keyDown(codeView.contentDOM, { ctrlKey: true, key: "Enter" })).toBe(false);
  expect(serializeKoshMarkdown(view.state.doc)).toContain("committed edit");
  expect(view.state.doc.childCount).toBe(childCount);
  expect(onChange).not.toHaveBeenCalled();

  act(() => {
    codeView.focus();
    codeView.dispatch({
      changes: { from: codeView.state.doc.length, insert: "\nunsafe edit" },
    });
  });
  expect(serializeKoshMarkdown(view.state.doc)).not.toContain("unsafe edit");
  expect(onChange).not.toHaveBeenCalled();
});

it("renders and toggles task state through accessible editor controls", () => {
  const { getAllByRole, view } = controlledEditor("- [x] done\n- [ ] later");
  const checkboxes = getAllByRole("checkbox");

  expect(checkboxes).toHaveLength(2);
  expect(checkboxes[0]).toBeChecked();
  expect(checkboxes[1]).not.toBeChecked();

  fireEvent.click(checkboxes[0]!);
  fireEvent.click(checkboxes[1]!);

  expect(serializeKoshMarkdown(view.state.doc)).toBe("- [ ] done\n- [x] later");
});

it("creates task items from typed list markers", () => {
  const { getByRole, view } = controlledEditor("");

  act(() => typeWithInputRules(view, "- [ ] capture this"));

  expect(getByRole("checkbox", { name: "Mark task complete" })).not.toBeChecked();
  expect(serializeKoshMarkdown(view.state.doc)).toBe("- [ ] capture this");
});

it("prevents table toolbar commands from creating unserializable cell blocks", () => {
  const { getByRole, textbox, view } = controlledEditor("| Heading |\n| ------- |\n| cell    |");

  expect(getByRole("button", { name: "Bulleted list" })).toBeDisabled();
  expect(getByRole("button", { name: "Block quote" })).toBeDisabled();

  const before = serializeKoshMarkdown(view.state.doc);
  expect(fireEvent.keyDown(textbox, { key: "Enter", shiftKey: true })).toBe(false);
  expect(serializeKoshMarkdown(view.state.doc)).toBe(before);
});

it("handles large plain-text paste, undo, and redo as editor transactions", () => {
  const onChange = vi.fn();
  const { getByRole } = render(<RichTextEditor ariaLabel="Body" onChange={onChange} value="" />);
  const textbox = getByRole("textbox", { name: "Body" });
  const view = editorView(textbox);
  const large = "knowledge ".repeat(15_000).trim();

  act(() => {
    view.focus();
    expect(view.pasteText(large)).toBe(true);
  });
  expect(view.state.doc.textContent).toBe(large);

  fireEvent.keyDown(textbox, { key: "z", metaKey: true });
  expect(view.state.doc.textContent).toBe("");
  fireEvent.keyDown(textbox, { key: "z", metaKey: true, shiftKey: true });
  expect(view.state.doc.textContent).toBe(large);
});

it("sanitizes pasted HTML through the Kosh schema", () => {
  const { textbox, view } = controlledEditor("");
  act(() => {
    view.focus();
    expect(
      view.pasteHTML(
        '<script>window.evil=true</script><a href="javascript:alert(1)">unsafe</a><img src="https://example.com/pixel.png">',
      ),
    ).toBe(true);
  });

  expect(textbox.querySelector("script, img, a")).toBeNull();
  expect(serializeKoshMarkdown(view.state.doc)).not.toContain("javascript:");
  expect(view.state.doc.textContent).toContain("unsafe");
});

it("round-trips image metadata, resizes from the keyboard, and removes the image", () => {
  const imageId = "01980c8e-6c00-7000-8000-000000000231";
  const onChange = vi.fn();
  const source =
    `{{kosh:image:${imageId};width=70%;alt=%2ASystem%2A%20%5Fdiagram%5F;` +
    "caption=%7E%7EChapter%7E%7E%20%28overview%29%21}}";
  const { getByLabelText, getByRole } = render(
    <RichTextEditor
      ariaLabel="Body"
      imageStatus={async () => ({
        attachmentId: imageId,
        naturalHeight: 800,
        naturalWidth: 1_200,
        nextAttemptAtMs: null,
        ocrError: null,
        ocrStatus: "READY",
      })}
      onChange={onChange}
      value={source}
    />,
  );
  const textbox = getByRole("textbox", { name: "Body" });
  const view = editorView(textbox);

  expect(getByRole("img", { name: "*System* _diagram_" })).toHaveAttribute(
    "src",
    `kosh-media://localhost/attachment/${imageId}`,
  );
  expect(getByLabelText("Alt text")).toHaveValue("*System* _diagram_");
  expect(getByLabelText("Caption")).toHaveValue("~~Chapter~~ (overview)!");

  fireEvent.input(getByLabelText("Alt text"), { target: { value: "Updated diagram" } });
  fireEvent.input(getByLabelText("Caption"), { target: { value: "Updated caption" } });
  act(() => {
    view.dispatch(view.state.tr.setSelection(NodeSelection.create(view.state.doc, 0)));
  });
  fireEvent.keyDown(textbox, { altKey: true, key: "ArrowLeft" });

  expect(serializeKoshMarkdown(view.state.doc)).toBe(
    `{{kosh:image:${imageId};width=65%;alt=Updated%20diagram;caption=Updated%20caption}}`,
  );
  fireEvent.click(getByRole("button", { name: "Remove" }));
  expect(serializeKoshMarkdown(view.state.doc)).toBe("");
});

it("enables existing image controls when draft loading completes", async () => {
  const imageId = "01980c8e-6c00-7000-8000-00000000024f";
  const source = `{{kosh:image:${imageId};width=70%;alt=Loaded%20image;caption=Loaded%20caption}}`;
  const imageStatus = vi.fn(async () => ({
    attachmentId: imageId,
    naturalHeight: 800,
    naturalWidth: 1_200,
    nextAttemptAtMs: null,
    ocrError: "Vision unavailable",
    ocrStatus: "FAILED" as const,
  }));
  const retryOcr = vi.fn(async () => ({
    attachmentId: imageId,
    naturalHeight: 800,
    naturalWidth: 1_200,
    nextAttemptAtMs: null,
    ocrError: null,
    ocrStatus: "PENDING" as const,
  }));
  const result = render(
    <RichTextEditor
      ariaLabel="Body"
      disabled
      imageStatus={imageStatus}
      onChange={() => undefined}
      retryImageOcr={retryOcr}
      value={source}
    />,
  );
  const altInput = result.getByLabelText("Alt text");
  const captionInput = result.getByLabelText("Caption");
  const remove = result.getByRole("button", { name: "Remove" });
  const retry = await result.findByRole("button", { name: "Retry text recognition" });
  expect(altInput).toBeDisabled();
  expect(captionInput).toBeDisabled();
  expect(remove).toBeDisabled();
  expect(retry).toBeDisabled();

  result.rerender(
    <RichTextEditor
      ariaLabel="Body"
      imageStatus={imageStatus}
      onChange={() => undefined}
      retryImageOcr={retryOcr}
      value={source}
    />,
  );

  expect(result.getByLabelText("Alt text")).toBe(altInput);
  expect(altInput).toBeEnabled();
  expect(captionInput).toBeEnabled();
  expect(remove).toBeEnabled();
  expect(retry).toBeEnabled();
});

it("shows a durable pending node while pasted image ingestion completes", async () => {
  let resolveImage!: (record: ImageRecord) => void;
  const imagePromise = new Promise<ImageRecord>((resolve) => {
    resolveImage = resolve;
  });
  const onPendingImagesChange = vi.fn();
  const onChange = vi.fn();
  const image = imageRecord("01980c8e-6c00-7000-8000-000000000232");
  const { container, getByRole } = render(
    <RichTextEditor
      ariaLabel="Body"
      onChange={onChange}
      onPendingImagesChange={onPendingImagesChange}
      pasteImage={() => imagePromise}
      value=""
    />,
  );
  const textbox = getByRole("textbox", { name: "Body" });

  fireEvent.paste(textbox, {
    clipboardData: { items: [{ type: "image/png" }] },
  });
  expect(getByRole("status", { name: "Processing pasted image" })).toHaveTextContent(
    "Processing pasted image",
  );
  expect(onPendingImagesChange).toHaveBeenLastCalledWith(true);

  await act(async () => resolveImage(image));
  await waitFor(() => {
    expect(container.querySelector("img")).toHaveAttribute(
      "src",
      `kosh-media://localhost/attachment/${image.id}`,
    );
  });
  expect(onPendingImagesChange).toHaveBeenLastCalledWith(false);
  expect(onChange).toHaveBeenLastCalledWith(`{{kosh:image:${image.id};width=100%}}`);
});

it("inserts images selected through the native picker action", async () => {
  const image = imageRecord("01980c8e-6c00-7000-8000-000000000235");
  const pickImage = vi.fn(async () => image);
  const { container, getByRole } = render(
    <RichTextEditor ariaLabel="Body" onChange={() => undefined} pickImage={pickImage} value="" />,
  );

  fireEvent.click(getByRole("button", { name: "Add image" }));

  expect(pickImage).toHaveBeenCalledOnce();
  await waitFor(() =>
    expect(container.querySelector("img")).toHaveAttribute(
      "src",
      `kosh-media://localhost/attachment/${image.id}`,
    ),
  );
});

it("inserts a PDF token and exposes extraction status and system-open controls", async () => {
  const pdf = pdfRecord("01980c8e-6c00-7000-8000-000000000236");
  const pickPdf = vi.fn(async () => pdf);
  const openPdfExternal = vi.fn(async () => undefined);
  const onChange = vi.fn();
  const { getByRole, getByText } = render(
    <RichTextEditor
      ariaLabel="Body"
      onChange={onChange}
      openPdfExternal={openPdfExternal}
      pdfStatus={async () => ({
        attachmentId: pdf.id,
        displayFilename: pdf.displayFilename,
        extractedPageCount: 2,
        extractionError: null,
        extractionStatus: "READY",
        nextAttemptAtMs: null,
        pageCount: 3,
        unavailablePageCount: 1,
      })}
      pickPdf={pickPdf}
      value=""
    />,
  );

  fireEvent.click(getByRole("button", { name: "Add PDF" }));

  expect(pickPdf).toHaveBeenCalledOnce();
  await waitFor(() => expect(getByText("chapter.pdf")).toBeVisible());
  expect(getByText("3 pages · 2 searchable · 1 unavailable")).toBeVisible();
  expect(onChange).toHaveBeenLastCalledWith(`{{kosh:pdf:${pdf.id}}}`);
  fireEvent.click(getByRole("button", { name: "Open original" }));
  await waitFor(() => expect(openPdfExternal).toHaveBeenCalledWith(pdf.id));
});

it("inserts, captions, opens, reveals, and replaces a generic attachment", async () => {
  const first = genericAttachmentRecord("01980c8e-6c00-7000-8000-000000000238", "notes.md");
  const second = genericAttachmentRecord(
    "01980c8e-6c00-7000-8000-000000000239",
    "archive.zip",
    "BINARY",
  );
  const pickAttachment = vi
    .fn()
    .mockResolvedValueOnce({ record: first, recordKind: "GENERIC" })
    .mockResolvedValueOnce({ record: second, recordKind: "GENERIC" });
  const openAttachmentExternal = vi.fn(async () => undefined);
  const revealAttachmentInFinder = vi.fn(async () => undefined);
  const onChange = vi.fn();
  const result = render(
    <RichTextEditor
      ariaLabel="Body"
      attachmentStatus={async (attachmentId) => {
        const record = attachmentId === first.id ? first : second;
        return {
          attachmentId,
          byteLength: record.byteLength,
          displayFilename: record.displayFilename,
          extractedLineCount: record.extractedLineCount,
          extractionError: record.extractionError,
          extractionStatus: record.extractionStatus,
          kind: record.kind,
          mediaType: record.mediaType,
        };
      }}
      onChange={onChange}
      openAttachmentExternal={openAttachmentExternal}
      pickAttachment={pickAttachment}
      revealAttachmentInFinder={revealAttachmentInFinder}
      value=""
    />,
  );

  fireEvent.click(result.getByRole("button", { name: "Add attachment" }));
  await result.findByText("notes.md");
  expect(result.getByText(/17 lines searchable/u)).toBeVisible();

  await userEvent.type(result.getByRole("textbox", { name: "Attachment caption" }), "Key notes");
  expect(onChange).toHaveBeenLastCalledWith(`{{kosh:attachment:${first.id};caption=Key%20notes}}`);
  fireEvent.click(result.getByRole("button", { name: "Open" }));
  fireEvent.click(result.getByRole("button", { name: "Reveal" }));
  await waitFor(() => {
    expect(openAttachmentExternal).toHaveBeenCalledWith(first.id);
    expect(revealAttachmentInFinder).toHaveBeenCalledWith(first.id);
  });

  fireEvent.click(result.getByRole("button", { name: "Replace" }));
  await result.findByText("archive.zip");
  expect(onChange).toHaveBeenLastCalledWith(`{{kosh:attachment:${second.id}}}`);
});

it("disables PDF extraction retries while the editor is disabled", async () => {
  const pdf = pdfRecord("01980c8e-6c00-7000-8000-000000000237");
  const retryPdfExtraction = vi.fn(async () => ({
    attachmentId: pdf.id,
    displayFilename: pdf.displayFilename,
    extractedPageCount: 0,
    extractionError: null,
    extractionStatus: "PENDING" as const,
    nextAttemptAtMs: null,
    pageCount: 3,
    unavailablePageCount: 0,
  }));
  const pdfStatus = vi.fn(async () => ({
    attachmentId: pdf.id,
    displayFilename: pdf.displayFilename,
    extractedPageCount: 0,
    extractionError: "PDF extraction failed",
    extractionStatus: "FAILED" as const,
    nextAttemptAtMs: null,
    pageCount: 3,
    unavailablePageCount: 0,
  }));
  const result = render(
    <RichTextEditor
      ariaLabel="Body"
      disabled
      onChange={() => undefined}
      pdfStatus={pdfStatus}
      retryPdfExtraction={retryPdfExtraction}
      value={`{{kosh:pdf:${pdf.id}}}`}
    />,
  );
  const retry = await result.findByRole("button", { name: "Retry extraction" });
  expect(retry).toBeDisabled();
  fireEvent.click(retry);
  expect(retryPdfExtraction).not.toHaveBeenCalled();

  result.rerender(
    <RichTextEditor
      ariaLabel="Body"
      onChange={() => undefined}
      pdfStatus={pdfStatus}
      retryPdfExtraction={retryPdfExtraction}
      value={`{{kosh:pdf:${pdf.id}}}`}
    />,
  );

  expect(retry).toBeEnabled();
  fireEvent.click(retry);
  await waitFor(() => expect(retryPdfExtraction).toHaveBeenCalledWith(pdf.id));
});

it("restores selected content when the native image picker is canceled", async () => {
  let resolvePick!: (record: ImageRecord | null) => void;
  const pickPromise = new Promise<ImageRecord | null>((resolve) => {
    resolvePick = resolve;
  });
  const value = "Keep this selected text";
  const onChange = vi.fn();
  const { getByRole, queryByRole } = render(
    <RichTextEditor
      ariaLabel="Body"
      onChange={onChange}
      pickImage={() => pickPromise}
      value={value}
    />,
  );
  const textbox = getByRole("textbox", { name: "Body" });
  const view = editorView(textbox);
  act(() => {
    view.dispatch(view.state.tr.setSelection(TextSelection.create(view.state.doc, 1, 5)));
  });

  fireEvent.click(getByRole("button", { name: "Add image" }));
  expect(getByRole("status", { name: "Choosing image" })).toBeVisible();
  expect(onChange).toHaveBeenLastCalledWith(value);
  act(() => {
    view.dispatch(view.state.tr.setSelection(TextSelection.atEnd(view.state.doc)).insertText("!"));
  });
  expect(onChange).toHaveBeenLastCalledWith(`${value}!`);
  await act(async () => resolvePick(null));

  await waitFor(() => expect(queryByRole("status", { name: "Choosing image" })).toBeNull());
  expect(serializeKoshMarkdown(view.state.doc)).toBe(`${value}!`);
});

it("removes failed image placeholders and supports explicit OCR retry", async () => {
  const imageId = "01980c8e-6c00-7000-8000-000000000233";
  const ingestError = new Error("invalid image");
  const onImageError = vi.fn();
  const onPendingImagesChange = vi.fn();
  const retryImageOcr = vi.fn(async () => ({
    attachmentId: imageId,
    naturalHeight: 800,
    naturalWidth: 1_200,
    nextAttemptAtMs: 100,
    ocrError: null,
    ocrStatus: "PENDING" as const,
  }));
  const { getByRole, queryByRole } = render(
    <RichTextEditor
      ariaLabel="Body"
      imageStatus={async () => ({
        attachmentId: imageId,
        naturalHeight: 800,
        naturalWidth: 1_200,
        nextAttemptAtMs: null,
        ocrError: "Vision unavailable",
        ocrStatus: "FAILED",
      })}
      onChange={() => undefined}
      onImageError={onImageError}
      onPendingImagesChange={onPendingImagesChange}
      pasteImage={() => Promise.reject(ingestError)}
      retryImageOcr={retryImageOcr}
      value={`{{kosh:image:${imageId};width=70%}}`}
    />,
  );
  const textbox = getByRole("textbox", { name: "Body" });
  await waitFor(() =>
    expect(getByRole("button", { name: "Retry text recognition" })).toBeVisible(),
  );
  fireEvent.click(getByRole("button", { name: "Retry text recognition" }));
  await waitFor(() => expect(retryImageOcr).toHaveBeenCalledWith(imageId));

  fireEvent.paste(textbox, {
    clipboardData: { items: [{ type: "image/png" }] },
  });
  await waitFor(() => expect(onImageError).toHaveBeenCalledWith(ingestError));
  expect(queryByRole("status", { name: "Processing pasted image" })).toBeNull();
  expect(onPendingImagesChange).toHaveBeenLastCalledWith(false);
  expect(serializeKoshMarkdown(editorView(textbox).state.doc)).toBe(
    `{{kosh:image:${imageId};width=70%}}`,
  );
});

it("backs off OCR status polling until a retry is eligible", () => {
  expect(statusPollDelay({ nextAttemptAtMs: 121_000, ocrStatus: "RETRY_WAIT" }, 1_000)).toBe(
    120_000,
  );
  expect(statusPollDelay({ nextAttemptAtMs: 3_601_000, ocrStatus: "RETRY_WAIT" }, 1_000)).toBe(
    300_000,
  );
  expect(statusPollDelay({ nextAttemptAtMs: null, ocrStatus: "RETRY_WAIT" }, 1_000)).toBe(300_000);
  expect(statusPollDelay({ nextAttemptAtMs: 1_000, ocrStatus: "PENDING" }, 1_000)).toBe(1_500);
});

it("does not duplicate transactions in React Strict Mode", () => {
  const onChange = vi.fn();
  const { getByRole } = render(
    <StrictMode>
      <RichTextEditor ariaLabel="Body" onChange={onChange} value="" />
    </StrictMode>,
  );
  const view = editorView(getByRole("textbox", { name: "Body" }));
  act(() => view.dispatch(view.state.tr.insertText("x")));
  expect(onChange).toHaveBeenCalledOnce();
});

function controlledEditor(value: string) {
  const result = render(
    <RichTextEditor ariaLabel="Editor" onChange={() => undefined} value={value} />,
  );
  const textbox = result.getByRole("textbox", { name: "Editor" });
  return {
    ...result,
    textbox,
    view: editorView(textbox),
  };
}

function imageRecord(id: string): ImageRecord {
  return {
    byteLength: 12_000,
    displayFilename: "knowledge.png",
    id,
    ingestLeaseId: "01980c8e-6c00-7000-8000-000000000234",
    kind: "IMAGE",
    mediaType: "image/png",
    naturalHeight: 800,
    naturalWidth: 1_200,
    ocrError: null,
    ocrStatus: "PENDING",
  };
}

function pdfRecord(id: string): PdfRecord {
  return {
    byteLength: 24_000,
    displayFilename: "chapter.pdf",
    extractionError: null,
    extractionStatus: "PENDING",
    id,
    ingestLeaseId: "01980c8e-6c00-7000-8000-000000000237",
    kind: "PDF",
    mediaType: "application/pdf",
    pageCount: 3,
  };
}

function genericAttachmentRecord(
  id: string,
  displayFilename: string,
  kind: "TEXT" | "BINARY" = "TEXT",
): GenericAttachmentRecord {
  return {
    byteLength: kind === "TEXT" ? 4_096 : 10_240,
    displayFilename,
    extractedLineCount: kind === "TEXT" ? 17 : 0,
    extractionError: null,
    extractionStatus: kind === "TEXT" ? "READY" : "NOT_APPLICABLE",
    id,
    ingestLeaseId: "01980c8e-6c00-7000-8000-000000000240",
    kind,
    mediaType: kind === "TEXT" ? "text/markdown" : "application/zip",
  };
}

function editorView(element: HTMLElement) {
  const view = richTextEditorViewFromDOM(element);
  if (!view) {
    throw new Error("Rich text editor view was not registered");
  }
  return view;
}

function typeWithInputRules(view: ReturnType<typeof editorView>, value: string) {
  view.focus();
  for (const character of value) {
    const { from, to } = view.state.selection;
    const handled = view.someProp("handleTextInput", (handler) =>
      handler(view, from, to, character),
    );
    if (!handled) {
      view.dispatch(view.state.tr.insertText(character, from, to));
    }
  }
}

async function embeddedCodeView(container: HTMLElement) {
  let codeView: CodeMirrorView | null = null;
  await waitFor(() => {
    const content = container.querySelector<HTMLElement>(".kosh-code-block-editor .cm-content");
    expect(content).not.toBeNull();
    codeView = content ? CodeMirrorView.findFromDOM(content) : null;
    expect(codeView).not.toBeNull();
  });
  return codeView!;
}

function expectWritingAssistanceDisabled(element: HTMLElement) {
  expect(element).toHaveAttribute("autocapitalize", "none");
  expect(element).toHaveAttribute("autocomplete", "off");
  expect(element).toHaveAttribute("autocorrect", "off");
  expect(element).toHaveAttribute("spellcheck", "false");
}

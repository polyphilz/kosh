import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createRef, useState } from "react";
import { describe, expect, it, vi } from "vitest";
import type { CitationResolution, SelectedAttachmentRecord } from "../../src/backend/contracts";
import { AppearanceProvider } from "../../src/components/Appearance";
import {
  KoshBlockNoteEditor,
  type KoshBlockNoteEditorHandle,
} from "../../src/editor/KoshBlockNoteEditor";

describe("production BlockNote editor", () => {
  it("loads and replaces controlled Markdown without reporting external changes", async () => {
    const onChange = vi.fn();
    const view = renderEditor("Initial **knowledge**.", onChange);

    expect(await screen.findByRole("textbox", { name: "Body" })).toHaveTextContent(
      "Initial knowledge.",
    );
    view.rerender(editorTree("Replacement with $x^2$.", onChange));

    await waitFor(() =>
      expect(screen.getByRole("textbox", { name: "Body" })).toHaveTextContent(
        "Replacement with x2.",
      ),
    );
    expect(onChange).not.toHaveBeenCalled();

    fireEvent.keyDown(screen.getByRole("textbox", { name: "Body" }), {
      key: "z",
      ctrlKey: true,
    });
    await waitFor(() =>
      expect(screen.getByRole("textbox", { name: "Body" })).toHaveTextContent(
        "Replacement with x2.",
      ),
    );
    expect(onChange).not.toHaveBeenCalled();
  });

  it("echoes rapid typing without dropping or reordering characters", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(
      <AppearanceProvider>
        <ControlledEditor onChange={onChange} />
      </AppearanceProvider>,
    );
    const textbox = await screen.findByRole("textbox", { name: "Body" });
    const paragraph = textbox.querySelector(".bn-inline-content") || textbox;

    await user.click(paragraph);
    await user.type(paragraph, "Do not lose this shower thought.");

    expect(textbox).toHaveTextContent("Do not lose this shower thought.");
    expect(onChange).toHaveBeenLastCalledWith("Do not lose this shower thought.");
  });

  it("exposes only the scoped insertion menu", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    renderEditor("", onChange);
    const textbox = await screen.findByRole("textbox", { name: "Body" });
    const paragraph = textbox.querySelector(".bn-inline-content") || textbox;

    await user.click(paragraph);
    await user.type(paragraph, "/");

    const menu = await screen.findByRole("listbox");
    expect(menu).toHaveTextContent("Heading 1");
    expect(menu).toHaveTextContent("Display math");
    expect(menu).not.toHaveTextContent("Table");
    expect(menu).not.toHaveTextContent("Audio");
    expect(onChange).not.toHaveBeenCalled();
    fireEvent.blur(textbox);
    await waitFor(() => expect(onChange).toHaveBeenLastCalledWith("/"));
  });

  it("turns off browser writing assistance for technical notes", async () => {
    renderEditor("", () => undefined);

    const textbox = await screen.findByRole("textbox", { name: "Body" });
    expect(textbox).toHaveAttribute("autocapitalize", "none");
    expect(textbox).toHaveAttribute("autocomplete", "off");
    expect(textbox).toHaveAttribute("autocorrect", "off");
    expect(textbox).toHaveAttribute("spellcheck", "false");
    expect(textbox).toHaveAttribute("writingsuggestions", "false");
  });

  it("inserts typed local media through its imperative bridge", async () => {
    const onChange = vi.fn();
    const ref = createRef<KoshBlockNoteEditorHandle>();
    render(
      <AppearanceProvider>
        <KoshBlockNoteEditor ariaLabel="Body" onChange={onChange} ref={ref} value="" />
      </AppearanceProvider>,
    );
    const attachment: SelectedAttachmentRecord = {
      recordKind: "GENERIC",
      record: {
        byteLength: 12,
        displayFilename: "memory.txt",
        extractedLineCount: 1,
        extractionError: null,
        extractionStatus: "READY",
        id: "019f547b-6200-7000-8000-000000000301",
        ingestLeaseId: "private-lease",
        kind: "TEXT",
        mediaType: "text/plain",
      },
    };

    act(() => ref.current?.insertAttachments([attachment]));

    expect(await screen.findByText("memory.txt")).toBeInTheDocument();
    await waitFor(() =>
      expect(onChange).toHaveBeenLastCalledWith(
        "{{kosh:attachment:019f547b-6200-7000-8000-000000000301}}",
      ),
    );
    expect(onChange.mock.calls.flat().join("\n")).not.toContain("private-lease");
  });

  it("focuses exact authored and media citation blocks without editing the note", async () => {
    const onChange = vi.fn();
    const ref = createRef<KoshBlockNoteEditorHandle>();
    render(
      <AppearanceProvider>
        <KoshBlockNoteEditor
          ariaLabel="Body"
          onChange={onChange}
          ref={ref}
          value={
            "# Slow recipe\n\nSlow simmering preserves brightness.\n\n{{kosh:image:019f547b-6200-7000-8000-000000000201;width=70%;alt=Diagram}}"
          }
        />
      </AppearanceProvider>,
    );
    await screen.findByText("Slow simmering preserves brightness.");

    expect(ref.current?.focusCitation(authoredCitation())).toBe(true);
    expect(document.querySelector('[data-kosh-search-hit="true"]')).toHaveTextContent(
      "Slow simmering preserves brightness.",
    );

    expect(ref.current?.focusCitation(mediaCitation())).toBe(true);
    expect(document.querySelector('[data-kosh-search-hit="true"]')).toContainElement(
      screen.getByLabelText("Image: Diagram"),
    );
    act(() => ref.current?.clearSearchFocus());
    expect(document.querySelector('[data-kosh-search-hit="true"]')).toBeNull();
    expect(onChange).not.toHaveBeenCalled();
  });

  it("refuses to retarget a citation whose excerpt is absent from the editor", async () => {
    const ref = createRef<KoshBlockNoteEditorHandle>();
    render(
      <AppearanceProvider>
        <KoshBlockNoteEditor
          ariaLabel="Body"
          onChange={() => undefined}
          ref={ref}
          value="A recovered working copy replaced the cited paragraph."
        />
      </AppearanceProvider>,
    );
    await screen.findByText("A recovered working copy replaced the cited paragraph.");

    expect(ref.current?.focusCitation(authoredCitation())).toBe(false);
    expect(document.querySelector('[data-kosh-search-hit="true"]')).toBeNull();
  });

  it("refuses to retarget a citation when its fallback excerpt is ambiguous", async () => {
    const ref = createRef<KoshBlockNoteEditorHandle>();
    render(
      <AppearanceProvider>
        <KoshBlockNoteEditor
          ariaLabel="Body"
          onChange={() => undefined}
          ref={ref}
          value={
            "An inserted block moved the citation.\n\nSlow simmering preserves brightness.\n\nSlow simmering preserves brightness."
          }
        />
      </AppearanceProvider>,
    );
    await screen.findAllByText("Slow simmering preserves brightness.");

    expect(ref.current?.focusCitation(authoredCitation())).toBe(false);
    expect(document.querySelector('[data-kosh-search-hit="true"]')).toBeNull();
  });

  it("clears focused evidence when an edit removes the cited passage", async () => {
    const user = userEvent.setup();
    const ref = createRef<KoshBlockNoteEditorHandle>();
    render(
      <AppearanceProvider>
        <KoshBlockNoteEditor
          ariaLabel="Body"
          onChange={() => undefined}
          ref={ref}
          value="Slow simmering preserves brightness."
        />
      </AppearanceProvider>,
    );
    const editor = await screen.findByRole("textbox", { name: "Body" });

    expect(ref.current?.focusCitation(authoredCitation())).toBe(true);
    await user.clear(editor);
    await user.type(editor, "Fast boiling changes everything.");

    expect(ref.current?.revalidateCitationFocus(authoredCitation())).toBe(false);
    expect(document.querySelector('[data-kosh-search-hit="true"]')).toBeNull();
  });

  it("clears stale focus without re-entering the controlled change callback", async () => {
    const user = userEvent.setup();
    const ref = createRef<KoshBlockNoteEditorHandle>();
    const onChange = vi.fn();
    function RevalidatingEditor() {
      const [value, setValue] = useState("Slow simmering preserves brightness.");
      return (
        <KoshBlockNoteEditor
          ariaLabel="Body"
          onChange={(nextValue) => {
            onChange(nextValue);
            setValue(nextValue);
            ref.current?.revalidateCitationFocus(authoredCitation());
          }}
          ref={ref}
          value={value}
        />
      );
    }
    render(
      <AppearanceProvider>
        <RevalidatingEditor />
      </AppearanceProvider>,
    );
    const editor = await screen.findByRole("textbox", { name: "Body" });

    expect(ref.current?.focusCitation(authoredCitation())).toBe(true);
    await user.clear(editor);
    await user.type(editor, "Fast boiling changes everything.");

    expect(onChange.mock.calls.length).toBeLessThan(40);
    expect(document.querySelector('[data-kosh-search-hit="true"]')).toBeNull();
  });

  it("preserves punctuation while validating citation evidence", async () => {
    const ref = createRef<KoshBlockNoteEditorHandle>();
    render(
      <AppearanceProvider>
        <KoshBlockNoteEditor
          ariaLabel="Body"
          onChange={() => undefined}
          ref={ref}
          value="The budget must remain > 5."
        />
      </AppearanceProvider>,
    );
    await screen.findByText("The budget must remain > 5.");
    const exactCitation: CitationResolution = {
      ...authoredCitation(),
      excerpt: "budget must remain > 5",
    };

    expect(ref.current?.focusCitation(exactCitation)).toBe(true);
    act(() => ref.current?.clearSearchFocus());
    expect(
      ref.current?.focusCitation({ ...exactCitation, excerpt: "budget must remain < 5" }),
    ).toBe(false);
    expect(document.querySelector('[data-kosh-search-hit="true"]')).toBeNull();
  });

  it("validates authored math with its citation delimiters", async () => {
    const ref = createRef<KoshBlockNoteEditorHandle>();
    render(
      <AppearanceProvider>
        <KoshBlockNoteEditor
          ariaLabel="Body"
          onChange={() => undefined}
          ref={ref}
          value={"Use `code` and $x < y$ when bounded.\n\n$$\na_i > 0\n$$"}
        />
      </AppearanceProvider>,
    );
    await screen.findByText(/Use/);
    const inlineCitation: CitationResolution = {
      ...authoredCitation(),
      excerpt: "Use `code` and $x < y$ when bounded.",
    };

    expect(ref.current?.focusCitation(inlineCitation)).toBe(true);
    act(() => ref.current?.clearSearchFocus());
    expect(
      ref.current?.focusCitation({
        ...inlineCitation,
        excerpt: "$$a_i > 0$$",
        locator: { ...inlineCitation.locator, kind: "MARKDOWN_BLOCKS", startBlock: 1, endBlock: 1 },
      }),
    ).toBe(true);
  });

  it("focuses exact character and line slices within long authored blocks", async () => {
    const ref = createRef<KoshBlockNoteEditorHandle>();
    const view = render(
      <AppearanceProvider>
        <KoshBlockNoteEditor
          ariaLabel="Body"
          onChange={() => undefined}
          ref={ref}
          value="zero αβ exact slice omega"
        />
      </AppearanceProvider>,
    );
    await screen.findByText("zero αβ exact slice omega");
    const characterCitation: CitationResolution = {
      ...authoredCitation(),
      excerpt: "exact slice",
      locator: {
        ...authoredCitation().locator,
        kind: "MARKDOWN_BLOCKS",
        startChar: 8,
        endChar: 19,
      },
    };

    expect(ref.current?.focusCitation(characterCitation)).toBe(true);
    expect(document.querySelector('[data-kosh-search-hit="true"]')).toHaveTextContent(
      "exact slice",
    );

    view.rerender(
      <AppearanceProvider>
        <KoshBlockNoteEditor
          ariaLabel="Body"
          onChange={() => undefined}
          ref={ref}
          value={"```python\nfirst\nsecond target\nthird\nfourth\n```\n"}
        />
      </AppearanceProvider>,
    );
    await screen.findByText(/second target/);
    const lineCitation: CitationResolution = {
      ...authoredCitation(),
      excerpt: "second target\nthird",
      locator: {
        ...authoredCitation().locator,
        kind: "MARKDOWN_BLOCKS",
        startLine: 2,
        endLine: 3,
      },
    };

    expect(ref.current?.focusCitation(lineCitation)).toBe(true);
    expect(document.querySelector('[data-kosh-search-hit="true"]')?.textContent).toBe(
      "second target\nthird",
    );

    const mismatchedLocator: CitationResolution = {
      ...characterCitation,
      locator: {
        ...characterCitation.locator,
        kind: "MARKDOWN_BLOCKS",
        startChar: 0,
        endChar: 4,
      },
    };
    expect(ref.current?.focusCitation(mismatchedLocator)).toBe(false);
    expect(document.querySelector('[data-kosh-search-hit="true"]')).toBeNull();
  });

  it("maps citation offsets across inline math evidence", async () => {
    const ref = createRef<KoshBlockNoteEditorHandle>();
    render(
      <AppearanceProvider>
        <KoshBlockNoteEditor
          ariaLabel="Body"
          onChange={() => undefined}
          ref={ref}
          value="zero $x < y$ exact slice omega"
        />
      </AppearanceProvider>,
    );
    await screen.findByText(/exact slice omega/);
    const citation: CitationResolution = {
      ...authoredCitation(),
      excerpt: "exact slice",
      locator: {
        ...authoredCitation().locator,
        kind: "MARKDOWN_BLOCKS",
        startChar: 13,
        endChar: 24,
      },
    };

    expect(ref.current?.focusCitation(citation)).toBe(true);
    expect(document.querySelector('[data-kosh-search-hit="true"]')).toHaveTextContent(
      "exact slice",
    );
  });

  it("makes disabled state explicit to assistive technology and BlockNote", async () => {
    const view = render(
      <AppearanceProvider>
        <KoshBlockNoteEditor
          ariaLabel="Body"
          disabled
          onChange={() => undefined}
          value={"Locked $x$.\n\n$$\ny\n$$\n\n> legacy"}
        />
      </AppearanceProvider>,
    );

    const textbox = await screen.findByRole("textbox", { name: "Body" });
    expect(textbox).toHaveAttribute("aria-disabled", "true");
    expect(textbox).toHaveAttribute("contenteditable", "false");
    expect(screen.getByLabelText("Inline math source")).toBeDisabled();
    expect(screen.getByLabelText("Display math source")).toBeDisabled();
    expect(screen.getByLabelText("Legacy Markdown source")).toBeDisabled();

    view.rerender(
      <AppearanceProvider>
        <KoshBlockNoteEditor
          ariaLabel="Body"
          onChange={() => undefined}
          value={"Locked $x$.\n\n$$\ny\n$$\n\n> legacy"}
        />
      </AppearanceProvider>,
    );
    await waitFor(() => expect(screen.getByLabelText("Inline math source")).toBeEnabled());
    expect(screen.getByLabelText("Display math source")).toBeEnabled();
    expect(screen.getByLabelText("Legacy Markdown source")).toBeEnabled();
  });

  it("tracks attachment replacement until the native ingest settles", async () => {
    let resolveReplacement!: (record: SelectedAttachmentRecord | null) => void;
    const replacement = new Promise<SelectedAttachmentRecord | null>((resolve) => {
      resolveReplacement = resolve;
    });
    const onPendingImagesChange = vi.fn();
    render(
      <AppearanceProvider>
        <KoshBlockNoteEditor
          ariaLabel="Body"
          onChange={() => undefined}
          onPendingImagesChange={onPendingImagesChange}
          pickAttachment={() => replacement}
          value="{{kosh:attachment:019f547b-6200-7000-8000-000000000301}}"
        />
      </AppearanceProvider>,
    );

    await userEvent.click(await screen.findByRole("button", { name: "Replace" }));
    expect(onPendingImagesChange).toHaveBeenLastCalledWith(true);

    act(() =>
      resolveReplacement({
        recordKind: "GENERIC",
        record: {
          byteLength: 7,
          displayFilename: "replacement.txt",
          extractedLineCount: 1,
          extractionError: null,
          extractionStatus: "READY",
          id: "019f547b-6200-7000-8000-000000000302",
          ingestLeaseId: "private-replacement-lease",
          kind: "TEXT",
          mediaType: "text/plain",
        },
      }),
    );

    expect(await screen.findByText("replacement.txt")).toBeInTheDocument();
    await waitFor(() => expect(onPendingImagesChange).toHaveBeenLastCalledWith(false));
  });

  it("rejects keyboard image resizing while locked", async () => {
    const onChange = vi.fn();
    render(
      <AppearanceProvider>
        <KoshBlockNoteEditor
          ariaLabel="Body"
          disabled
          onChange={onChange}
          value="{{kosh:image:019f547b-6200-7000-8000-000000000201;width=70%;alt=Diagram}}"
        />
      </AppearanceProvider>,
    );

    fireEvent.keyDown(await screen.findByLabelText("Image: Diagram"), {
      altKey: true,
      key: "ArrowRight",
    });
    expect(onChange).not.toHaveBeenCalled();
  });
});

function ControlledEditor({ onChange }: { onChange: (value: string) => void }) {
  const [value, setValue] = useState("");
  return (
    <KoshBlockNoteEditor
      ariaLabel="Body"
      onChange={(nextValue) => {
        onChange(nextValue);
        setValue(nextValue);
      }}
      value={value}
    />
  );
}

function renderEditor(value: string, onChange: (value: string) => void) {
  return render(editorTree(value, onChange));
}

function editorTree(value: string, onChange: (value: string) => void) {
  return (
    <AppearanceProvider>
      <KoshBlockNoteEditor ariaLabel="Body" onChange={onChange} value={value} />
    </AppearanceProvider>
  );
}

function authoredCitation(): CitationResolution {
  return {
    passageId: "passage-authored",
    excerpt: "Slow simmering preserves brightness.",
    headingContext: ["Slow recipe"],
    constructionVersion: "test-v1",
    state: "CURRENT",
    locator: {
      kind: "MARKDOWN_BLOCKS",
      startBlock: 0,
      endBlock: 0,
      sourceStartByte: null,
      sourceEndByte: null,
      startChar: null,
      endChar: null,
      startLine: null,
      endLine: null,
    },
    tidbit: null,
    attachment: null,
    sources: [],
  };
}

function mediaCitation(): CitationResolution {
  return {
    passageId: "passage-image",
    excerpt: "Diagram",
    headingContext: ["Slow recipe"],
    constructionVersion: "test-v1",
    state: "CURRENT",
    locator: { kind: "OCR_REGION", page: null, region: null },
    tidbit: null,
    attachment: {
      id: "019f547b-6200-7000-8000-000000000201",
      extractionId: "extraction-image",
      displayFilename: "diagram.png",
      mediaType: "image/png",
      deleted: false,
    },
    sources: [],
  };
}

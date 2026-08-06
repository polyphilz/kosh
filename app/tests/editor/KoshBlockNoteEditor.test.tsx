import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createRef, forwardRef, useState, type ComponentProps } from "react";
import { describe, expect, it, vi } from "vitest";
import type { SelectedAttachmentRecord } from "../../src/backend/contracts";
import { AppearanceProvider } from "../../src/components/Appearance";
import {
  KoshBlockNoteEditor as ProductionKoshBlockNoteEditor,
  type KoshBlockNoteEditorHandle,
} from "../../src/editor/KoshBlockNoteEditor";
import { createKoshDocumentFromMarkdown } from "../../src/editor/document";

const legacyDocumentCache = new Map<string, string>();
type TestEditorProps = Omit<
  ComponentProps<typeof ProductionKoshBlockNoteEditor>,
  "onChange" | "value"
> & {
  onChange: (bodyMarkdown: string) => void;
  value: string;
};
const KoshBlockNoteEditor = forwardRef<KoshBlockNoteEditorHandle, TestEditorProps>(
  function TestKoshBlockNoteEditor({ onChange, value, ...properties }, ref) {
    let documentJson = legacyDocumentCache.get(value);
    if (!documentJson) {
      documentJson = createKoshDocumentFromMarkdown(value);
      legacyDocumentCache.set(value, documentJson);
    }
    return (
      <ProductionKoshBlockNoteEditor
        {...properties}
        onChange={(nextDocumentJson, bodyMarkdown) => {
          legacyDocumentCache.set(bodyMarkdown, nextDocumentJson);
          onChange(bodyMarkdown);
        }}
        ref={ref}
        value={documentJson}
      />
    );
  },
);

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

  it("emits a user edit that returns to the frozen initial value", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    renderEditor("", onChange);
    const textbox = await screen.findByRole("textbox", { name: "Body" });
    const paragraph = textbox.querySelector(".bn-inline-content") || textbox;

    await user.click(paragraph);
    await user.type(paragraph, "f");
    await user.clear(textbox);

    expect(textbox.textContent).toBe("");
    expect(onChange).toHaveBeenLastCalledWith("");
  });

  it("emits restoration of an existing note's exact initial content", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    renderEditor("Original thought.", onChange);
    const textbox = await screen.findByRole("textbox", { name: "Body" });

    await user.clear(textbox);
    await user.type(textbox, "Original thought.");

    expect(textbox).toHaveTextContent("Original thought.");
    expect(onChange).toHaveBeenLastCalledWith("Original thought.");
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
      recordKind: "FILE",
      record: {
        byteLength: 12,
        displayFilename: "memory.txt",
        id: "019f547b-6200-7000-8000-000000000301",
        ingestLeaseId: "private-lease",
        kind: "FILE",
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

  it("focuses a stable block id without editing the note", async () => {
    const onChange = vi.fn();
    const ref = createRef<KoshBlockNoteEditorHandle>();
    const blockId = "019f547b-6200-7000-8000-000000000401";
    render(
      <AppearanceProvider>
        <ProductionKoshBlockNoteEditor
          ariaLabel="Body"
          onChange={onChange}
          ref={ref}
          value={JSON.stringify({
            schemaVersion: 1,
            blocks: [
              {
                id: blockId,
                type: "paragraph",
                props: {},
                content: [
                  { type: "text", text: "Slow simmering preserves brightness.", styles: {} },
                ],
              },
            ],
          })}
        />
      </AppearanceProvider>,
    );
    await screen.findByText("Slow simmering preserves brightness.");

    expect(ref.current?.hasBlock(blockId)).toBe(true);
    expect(ref.current?.focusBlock(blockId)).toBe(true);
    expect(document.querySelector('[data-kosh-search-hit="true"]')).toHaveTextContent(
      "Slow simmering preserves brightness.",
    );
    act(() => ref.current?.clearSearchFocus());
    expect(document.querySelector('[data-kosh-search-hit="true"]')).toBeNull();
    expect(ref.current?.focusBlock("missing-block")).toBe(false);
    expect(onChange).not.toHaveBeenCalled();
  });

  it("finds case-insensitive text across rich marks without editing", async () => {
    const onChange = vi.fn();
    const ref = createRef<KoshBlockNoteEditorHandle>();
    render(
      <AppearanceProvider>
        <KoshBlockNoteEditor
          ariaLabel="Body"
          onChange={onChange}
          ref={ref}
          value={"Needle once.\n\nSecond **nee**dle and NEEDLE.\n\n$$\nmatrix_token\n$$"}
        />
      </AppearanceProvider>,
    );
    await waitFor(() =>
      expect(screen.getByRole("textbox", { name: "Body" })).toHaveTextContent(
        "Second needle and NEEDLE.",
      ),
    );

    let result: { activeIndex: number; count: number } | undefined;
    act(() => {
      result = ref.current?.findInNote("needle");
    });
    expect(result).toEqual({ activeIndex: 0, count: 3 });
    expect(document.querySelectorAll('[data-kosh-find-active="true"]')).toHaveLength(1);

    act(() => {
      result = ref.current?.moveFindInNote("next");
    });
    expect(result).toEqual({ activeIndex: 1, count: 3 });
    expect(
      [...document.querySelectorAll('[data-kosh-find-active="true"]')]
        .map((element) => element.textContent)
        .join(""),
    ).toBe("needle");

    act(() => {
      result = ref.current?.findInNote("MATRIX_TOKEN");
    });
    expect(result).toEqual({ activeIndex: 0, count: 1 });
    expect(document.querySelectorAll('[data-kosh-find-active="true"]')).toHaveLength(1);

    act(() => ref.current?.clearFindInNote());
    expect(document.querySelector('[data-kosh-find-match="true"]')).toBeNull();
    expect(onChange).not.toHaveBeenCalled();
  });

  it("counts each atomic math node once when its source repeats the query", async () => {
    const ref = createRef<KoshBlockNoteEditorHandle>();
    render(
      <AppearanceProvider>
        <KoshBlockNoteEditor
          ariaLabel="Body"
          onChange={() => undefined}
          ref={ref}
          value={"Inline $token + token$.\n\n$$\ntoken + token\n$$"}
        />
      </AppearanceProvider>,
    );
    await screen.findByRole("button", { name: "Edit inline math: token + token" });

    let result: { activeIndex: number; count: number } | undefined;
    act(() => {
      result = ref.current?.findInNote("token");
    });
    expect(result).toEqual({ activeIndex: 0, count: 2 });
    expect(document.querySelectorAll('[data-kosh-find-match="true"]')).toHaveLength(2);
    expect(document.querySelectorAll('[data-kosh-find-active="true"]')).toHaveLength(1);

    act(() => {
      result = ref.current?.moveFindInNote("next");
    });
    expect(result).toEqual({ activeIndex: 1, count: 2 });
    expect(document.querySelectorAll('[data-kosh-find-active="true"]')).toHaveLength(1);
  });

  it("does not match across an atomic math boundary", async () => {
    const ref = createRef<KoshBlockNoteEditorHandle>();
    render(
      <AppearanceProvider>
        <KoshBlockNoteEditor
          ariaLabel="Body"
          onChange={() => undefined}
          ref={ref}
          value="Inline $ababa$b"
        />
      </AppearanceProvider>,
    );
    await screen.findByRole("button", { name: "Edit inline math: ababa" });

    expect(ref.current?.findInNote("ab")).toEqual({ activeIndex: 0, count: 1 });
    expect(document.querySelectorAll('[data-kosh-find-match="true"]')).toHaveLength(1);
  });

  it("finds visible media filenames, alt text, and captions", async () => {
    const ref = createRef<KoshBlockNoteEditorHandle>();
    render(
      <AppearanceProvider>
        <KoshBlockNoteEditor
          ariaLabel="Body"
          onChange={() => undefined}
          ref={ref}
          value={
            "{{kosh:image:019f547b-6200-7000-8000-000000000201;width=70%;alt=Architecture%20diagram;caption=Chapter%20overview}}"
          }
        />
      </AppearanceProvider>,
    );
    await screen.findByLabelText("Alt text");

    act(() =>
      ref.current?.insertAttachments([
        {
          recordKind: "FILE",
          record: {
            byteLength: 128,
            displayFilename: "matrix-notes.csv",
            id: "019f547b-6200-7000-8000-000000000302",
            ingestLeaseId: "private-csv-lease",
            kind: "FILE",
            mediaType: "text/csv",
          },
        },
        {
          recordKind: "FILE",
          record: {
            byteLength: 12,
            displayFilename: "memory.txt",
            id: "019f547b-6200-7000-8000-000000000303",
            ingestLeaseId: "private-file-lease",
            kind: "FILE",
            mediaType: "text/plain",
          },
        },
      ]),
    );
    await screen.findByText("matrix-notes.csv");
    const memoryFilename = await screen.findByText("memory.txt");
    const memoryFile = memoryFilename.closest("[data-kosh-file='true']");
    if (!memoryFile) throw new Error("memory file block is unavailable");
    fireEvent.change(within(memoryFile).getByLabelText("Attachment caption"), {
      target: { value: "Memory appendix" },
    });

    expect(ref.current?.findInNote("Architecture diagram")).toEqual({
      activeIndex: 0,
      count: 1,
    });
    expect(ref.current?.findInNote("Chapter overview")).toEqual({ activeIndex: 0, count: 1 });
    expect(ref.current?.findInNote("matrix-notes.csv")).toEqual({ activeIndex: 0, count: 1 });
    expect(ref.current?.findInNote("memory.txt")).toEqual({ activeIndex: 0, count: 1 });
    expect(ref.current?.findInNote("Memory appendix")).toEqual({ activeIndex: 0, count: 1 });
  });

  it("makes disabled state explicit to assistive technology and BlockNote", async () => {
    const view = render(
      <AppearanceProvider>
        <KoshBlockNoteEditor
          ariaLabel="Body"
          disabled
          onChange={() => undefined}
          value={"Locked $x$.\n\n$$\ny\n$$"}
        />
      </AppearanceProvider>,
    );

    const textbox = await screen.findByRole("textbox", { name: "Body" });
    expect(textbox).toHaveAttribute("aria-disabled", "true");
    expect(textbox).toHaveAttribute("contenteditable", "false");
    expect(screen.queryByLabelText("Inline math source")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Edit inline math: x" })).toHaveAttribute(
      "aria-disabled",
      "true",
    );
    expect(screen.getByLabelText("Display math source")).toBeDisabled();

    view.rerender(
      <AppearanceProvider>
        <KoshBlockNoteEditor
          ariaLabel="Body"
          onChange={() => undefined}
          value={"Locked $x$.\n\n$$\ny\n$$"}
        />
      </AppearanceProvider>,
    );
    const inlineMath = screen.getByRole("button", { name: "Edit inline math: x" });
    await waitFor(() => expect(inlineMath).toHaveAttribute("aria-disabled", "false"));
    fireEvent.click(inlineMath);
    expect(await screen.findByLabelText("Inline math source")).toBeEnabled();
    expect(screen.getByLabelText("Display math source")).toBeEnabled();
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

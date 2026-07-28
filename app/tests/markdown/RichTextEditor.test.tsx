import { EditorView as CodeMirrorView } from "@codemirror/view";
import { act, fireEvent, render, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StrictMode, createRef } from "react";
import { TextSelection } from "prosemirror-state";
import { expect, it, vi } from "vitest";
import { koshEditorSchema } from "../../src/markdown/editorSchema";
import { richTextEditorViewFromDOM } from "../../src/markdown/editorViewRegistry";
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

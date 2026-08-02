import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createRef, useState } from "react";
import { describe, expect, it, vi } from "vitest";
import type { SelectedAttachmentRecord } from "../../src/backend/contracts";
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

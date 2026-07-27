import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import { Button } from "../../src/components/Button";
import { Dialog } from "../../src/components/Dialog";
import { Select } from "../../src/components/Select";
import { Toggle } from "../../src/components/Toggle";

describe("shared controls", () => {
  it("operates the toggle from the keyboard", async () => {
    const user = userEvent.setup();

    function Harness() {
      const [checked, setChecked] = useState(false);
      return <Toggle checked={checked} label="Semantic retrieval" onChange={setChecked} />;
    }

    render(<Harness />);
    const toggle = screen.getByRole("switch", { name: "Semantic retrieval" });
    toggle.focus();
    await user.keyboard(" ");

    expect(toggle).toHaveAttribute("aria-checked", "true");
  });

  it("reports native select changes", async () => {
    const user = userEvent.setup();
    const onValueChange = vi.fn();

    render(
      <Select
        aria-label="Search mode"
        onValueChange={onValueChange}
        options={[
          { label: "Hybrid", value: "HYBRID" },
          { label: "Exact", value: "EXACT" },
        ]}
        value="HYBRID"
      />,
    );

    await user.selectOptions(screen.getByRole("combobox"), "EXACT");
    expect(onValueChange).toHaveBeenCalledWith("EXACT");
  });

  it("traps focus, closes on Escape, and restores the trigger", async () => {
    const user = userEvent.setup();

    function Harness() {
      const [open, setOpen] = useState(false);
      return (
        <>
          <Button onClick={() => setOpen(true)}>Open confirmation</Button>
          <Dialog
            footer={
              <>
                <Button data-autofocus onClick={() => setOpen(false)}>
                  Cancel
                </Button>
                <Button variant="danger">Remove</Button>
              </>
            }
            onClose={() => setOpen(false)}
            open={open}
            title="Remove source?"
          >
            <p>Only the source link is removed.</p>
          </Dialog>
        </>
      );
    }

    render(<Harness />);
    const trigger = screen.getByRole("button", { name: "Open confirmation" });
    await user.click(trigger);
    const cancel = await screen.findByRole("button", { name: "Cancel" });
    const remove = screen.getByRole("button", { name: "Remove" });
    await waitFor(() => expect(cancel).toHaveFocus());

    await user.tab({ shift: true });
    expect(remove).toHaveFocus();
    await user.tab();
    expect(cancel).toHaveFocus();

    const dialog = screen.getByRole("dialog");
    dialog.focus();
    await user.tab({ shift: true });
    expect(remove).toHaveFocus();

    await user.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
  });
});

import { describe, expect, it } from "vitest";
import {
  acceleratorForKeyboardEvent,
  acceleratorKeys,
  describeAccelerator,
  formatAccelerator,
} from "../../src/shortcuts/accelerator";

describe("global shortcut accelerators", () => {
  it("captures stable physical key codes and formats macOS labels", () => {
    expect(
      acceleratorForKeyboardEvent({
        altKey: true,
        code: "KeyT",
        ctrlKey: true,
        metaKey: true,
        shiftKey: false,
      }),
    ).toBe("control+alt+super+KeyT");
    expect(acceleratorKeys("control+alt+super+KeyT")).toEqual(["⌃", "⌥", "⌘", "T"]);
    expect(formatAccelerator("control+alt+super+KeyT")).toBe("⌃⌥⌘T");
    expect(describeAccelerator("control+alt+super+KeyT")).toBe("Control Option Command T");
  });

  it("rejects bare and unsupported keys before native registration", () => {
    expect(
      acceleratorForKeyboardEvent({
        altKey: false,
        code: "KeyT",
        ctrlKey: false,
        metaKey: false,
        shiftKey: false,
      }),
    ).toEqual({ message: "Include at least one modifier key." });
    expect(
      acceleratorForKeyboardEvent({
        altKey: false,
        code: "AudioVolumeUp",
        ctrlKey: true,
        metaKey: false,
        shiftKey: false,
      }),
    ).toEqual({ message: "That key cannot be used as a global shortcut." });
  });
});

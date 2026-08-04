import { fireEvent, render } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { KoshText } from "../../src/components/KoshText";
import {
  KoshTextTone,
  KoshTextVariant,
  type KoshTextTone as KoshTextToneType,
  type KoshTextVariant as KoshTextVariantType,
} from "../../src/components/kosh-text-types";

const variantClasses: Record<KoshTextVariantType, string> = {
  [KoshTextVariant.Title]: "kosh-text-title",
  [KoshTextVariant.Heading]: "kosh-text-heading",
  [KoshTextVariant.Subheading]: "kosh-text-subheading",
  [KoshTextVariant.Body]: "kosh-text-body",
  [KoshTextVariant.Supporting]: "kosh-text-supporting",
  [KoshTextVariant.Label]: "kosh-text-label",
  [KoshTextVariant.Caption]: "kosh-text-caption",
  [KoshTextVariant.Eyebrow]: "kosh-text-eyebrow",
};

const toneClasses: Record<KoshTextToneType, string> = {
  [KoshTextTone.Default]: "kosh-text-tone-default",
  [KoshTextTone.Muted]: "kosh-text-tone-muted",
  [KoshTextTone.Accent]: "kosh-text-tone-accent",
  [KoshTextTone.Success]: "kosh-text-tone-success",
  [KoshTextTone.Warning]: "kosh-text-tone-warning",
  [KoshTextTone.Danger]: "kosh-text-tone-danger",
  [KoshTextTone.Inherit]: "kosh-text-tone-inherit",
};

test("separates semantic markup from visual hierarchy", () => {
  const { getByRole } = render(
    <KoshText as="h2" variant={KoshTextVariant.Body}>
      Offsite recovery
    </KoshText>,
  );

  expect(getByRole("heading", { level: 2 })).toHaveClass("kosh-text-body");
});

test.each(Object.values(KoshTextVariant))("maps the %s variant to one role class", (variant) => {
  const { getByText } = render(
    <KoshText as="span" variant={variant}>
      Specimen
    </KoshText>,
  );

  expect(getByText("Specimen")).toHaveClass("kosh-text", variantClasses[variant]);
});

test.each(Object.values(KoshTextTone))("maps the %s tone to one tone class", (tone) => {
  const { getByText } = render(
    <KoshText as="span" tone={tone} variant={KoshTextVariant.Supporting}>
      Specimen
    </KoshText>,
  );

  expect(getByText("Specimen")).toHaveClass(toneClasses[tone]);
});

test("defaults the tone and composes a caller layout class", () => {
  const { getByText } = render(
    <KoshText as="p" className="layout-copy" variant={KoshTextVariant.Supporting}>
      Local-first notes
    </KoshText>,
  );

  expect(getByText("Local-first notes")).toHaveClass("kosh-text-tone-default", "layout-copy");
});

test("forwards semantic, ARIA, data, and event attributes", () => {
  const onClick = vi.fn();
  const { getByTestId } = render(
    <KoshText
      aria-live="polite"
      as="output"
      data-testid="backup-status"
      id="backup-status"
      onClick={onClick}
      tone={KoshTextTone.Success}
      variant={KoshTextVariant.Caption}
    >
      Recovery point ready
    </KoshText>,
  );

  const status = getByTestId("backup-status");
  expect(status.tagName).toBe("OUTPUT");
  expect(status).toHaveAttribute("aria-live", "polite");
  fireEvent.click(status);
  expect(onClick).toHaveBeenCalledTimes(1);
});

test("preserves associated label semantics without adding a focus stop", () => {
  const { getByLabelText, getByText } = render(
    <div>
      <KoshText as="label" htmlFor="bucket" variant={KoshTextVariant.Label}>
        Bucket
      </KoshText>
      <input id="bucket" />
      <KoshText as="p" variant={KoshTextVariant.Body}>
        Ordinary copy
      </KoshText>
    </div>,
  );

  expect(getByLabelText("Bucket")).toBeTruthy();
  expect(getByText("Ordinary copy")).not.toHaveAttribute("tabindex");
});

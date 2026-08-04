export const KoshTextVariant = {
  Title: "TITLE",
  Heading: "HEADING",
  Subheading: "SUBHEADING",
  Body: "BODY",
  Supporting: "SUPPORTING",
  Label: "LABEL",
  Caption: "CAPTION",
  Eyebrow: "EYEBROW",
} as const;

export type KoshTextVariant = (typeof KoshTextVariant)[keyof typeof KoshTextVariant];

export const KoshTextTone = {
  Default: "DEFAULT",
  Muted: "MUTED",
  Accent: "ACCENT",
  Success: "SUCCESS",
  Warning: "WARNING",
  Danger: "DANGER",
  Inherit: "INHERIT",
} as const;

export type KoshTextTone = (typeof KoshTextTone)[keyof typeof KoshTextTone];

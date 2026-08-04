import type { ComponentPropsWithoutRef, ElementType } from "react";
import { classNames } from "../lib/classNames";
import {
  KoshTextTone,
  KoshTextVariant,
  type KoshTextTone as KoshTextToneType,
  type KoshTextVariant as KoshTextVariantType,
} from "./kosh-text-types";
import "./kosh-text.css";

export type KoshTextElement =
  | "div"
  | "span"
  | "p"
  | "h1"
  | "h2"
  | "h3"
  | "h4"
  | "label"
  | "legend"
  | "small"
  | "strong"
  | "q"
  | "output"
  | "dt"
  | "dd";

export type KoshTextProps<TElement extends KoshTextElement> = Omit<
  ComponentPropsWithoutRef<TElement>,
  "className" | "style"
> & {
  as: TElement;
  className?: string;
  tone?: KoshTextToneType;
  variant: KoshTextVariantType;
};

const variantClass: Record<KoshTextVariantType, string> = {
  [KoshTextVariant.Title]: "kosh-text-title",
  [KoshTextVariant.Heading]: "kosh-text-heading",
  [KoshTextVariant.Subheading]: "kosh-text-subheading",
  [KoshTextVariant.Body]: "kosh-text-body",
  [KoshTextVariant.Supporting]: "kosh-text-supporting",
  [KoshTextVariant.Label]: "kosh-text-label",
  [KoshTextVariant.Caption]: "kosh-text-caption",
  [KoshTextVariant.Eyebrow]: "kosh-text-eyebrow",
};

const toneClass: Record<KoshTextToneType, string> = {
  [KoshTextTone.Default]: "kosh-text-tone-default",
  [KoshTextTone.Muted]: "kosh-text-tone-muted",
  [KoshTextTone.Accent]: "kosh-text-tone-accent",
  [KoshTextTone.Success]: "kosh-text-tone-success",
  [KoshTextTone.Warning]: "kosh-text-tone-warning",
  [KoshTextTone.Danger]: "kosh-text-tone-danger",
  [KoshTextTone.Inherit]: "kosh-text-tone-inherit",
};

export function KoshText<TElement extends KoshTextElement>({
  as,
  className,
  tone = KoshTextTone.Default,
  variant,
  ...props
}: KoshTextProps<TElement>) {
  const Element = as as ElementType;

  return (
    <Element
      {...props}
      className={classNames("kosh-text", variantClass[variant], toneClass[tone], className)}
    />
  );
}

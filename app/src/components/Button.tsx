import { forwardRef, type ComponentPropsWithoutRef } from "react";
import { classNames } from "../lib/classNames";

export type ButtonVariant = "surface" | "ghost" | "primary" | "accent" | "danger";
export type ButtonSize = "standard" | "compact" | "icon";

export interface ButtonProps extends ComponentPropsWithoutRef<"button"> {
  size?: ButtonSize;
  variant?: ButtonVariant;
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  { className, size = "standard", type = "button", variant = "surface", ...props },
  ref,
) {
  return (
    <button
      {...props}
      className={classNames(
        "kosh-button",
        `kosh-button--${size}`,
        `kosh-button--${variant}`,
        className,
      )}
      ref={ref}
      type={type}
    />
  );
});

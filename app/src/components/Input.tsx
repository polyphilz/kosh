import { forwardRef, type ComponentPropsWithoutRef } from "react";
import { classNames } from "../lib/classNames";

export type InputProps = ComponentPropsWithoutRef<"input">;

export const Input = forwardRef<HTMLInputElement, InputProps>(function Input(
  { className, ...props },
  ref,
) {
  return <input {...props} className={classNames("kosh-input", className)} ref={ref} />;
});

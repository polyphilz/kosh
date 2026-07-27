import type { SelectHTMLAttributes } from "react";
import { classNames } from "../lib/classNames";

export interface SelectOption<Value extends string> {
  label: string;
  value: Value;
}

interface SelectProps<Value extends string> extends Omit<
  SelectHTMLAttributes<HTMLSelectElement>,
  "children" | "onChange" | "value"
> {
  onValueChange: (value: Value) => void;
  options: readonly SelectOption<Value>[];
  value: Value;
}

export function Select<Value extends string>({
  className,
  onValueChange,
  options,
  value,
  ...props
}: SelectProps<Value>) {
  return (
    <span className={classNames("kosh-select", className)}>
      <select
        {...props}
        onChange={(event) => onValueChange(event.currentTarget.value as Value)}
        value={value}
      >
        {options.map((option) => (
          <option key={option.value} value={option.value}>
            {option.label}
          </option>
        ))}
      </select>
      <svg aria-hidden="true" viewBox="0 0 10 6">
        <path d="M1 1 5 5 9 1" />
      </svg>
    </span>
  );
}

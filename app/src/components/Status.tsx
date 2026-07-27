import { classNames } from "../lib/classNames";

type StatusTone = "neutral" | "success" | "warning" | "danger";

interface StatusProps {
  children: string;
  live?: boolean;
  tone?: StatusTone;
}

export function Status({ children, live = false, tone = "neutral" }: StatusProps) {
  return (
    <span
      aria-live={live ? "polite" : undefined}
      className={classNames("kosh-status", `kosh-status--${tone}`)}
      role={live ? "status" : undefined}
    >
      <span aria-hidden="true" />
      {children}
    </span>
  );
}

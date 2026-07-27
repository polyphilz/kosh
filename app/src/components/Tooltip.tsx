import { Children, cloneElement, useId, type ReactElement } from "react";
import { classNames } from "../lib/classNames";

interface TooltipTargetProps {
  "aria-describedby"?: string;
}

interface TooltipProps {
  children: ReactElement<TooltipTargetProps>;
  content: string;
  forceOpen?: boolean;
}

export function Tooltip({ children, content, forceOpen = false }: TooltipProps) {
  const id = useId();
  const target = Children.only(children);
  const describedBy = [target.props["aria-describedby"], id].filter(Boolean).join(" ");

  return (
    <span className={classNames("kosh-tooltip", forceOpen && "kosh-tooltip--open")}>
      {cloneElement(target, { "aria-describedby": describedBy })}
      <span id={id} role="tooltip">
        {content}
      </span>
    </span>
  );
}

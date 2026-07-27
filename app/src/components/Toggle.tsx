import { Button } from "./Button";

interface ToggleProps {
  checked: boolean;
  disabled?: boolean;
  label: string;
  onChange: (checked: boolean) => void;
}

export function Toggle({ checked, disabled = false, label, onChange }: ToggleProps) {
  return (
    <Button
      aria-checked={checked}
      aria-label={label}
      className="kosh-toggle"
      disabled={disabled}
      onClick={() => onChange(!checked)}
      role="switch"
      size="icon"
      variant="ghost"
    >
      <span aria-hidden="true" />
    </Button>
  );
}

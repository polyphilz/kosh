interface ShortcutProps {
  keys: readonly string[];
  label: string;
}

export function Shortcut({ keys, label }: ShortcutProps) {
  return (
    <span aria-label={label} className="kosh-shortcut" role="img">
      {keys.map((key) => (
        <kbd aria-hidden="true" key={key}>
          {key}
        </kbd>
      ))}
    </span>
  );
}

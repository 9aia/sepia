import { Kbd } from "../ui/kbd";

interface ShortcutsFooterProps {
  readonly modKey: string;
}

export function ShortcutsFooter({ modKey }: ShortcutsFooterProps) {
  const item = "inline-flex items-center gap-1";
  return (
    <footer className="flex flex-wrap gap-x-3 gap-y-1.5 border-t border-border px-3 py-2 text-[11px] text-muted-foreground">
      <span className={item}>
        <Kbd>↑</Kbd>
        <Kbd>↓</Kbd> navigate
      </span>
      <span className={item}>
        <Kbd>N</Kbd> new session
      </span>
      <span className={item}>
        <Kbd>Esc</Kbd> clear filter
      </span>
      <span className={item}>
        <Kbd>{modKey}</Kbd>
        <Kbd>B</Kbd> sidebar
      </span>
    </footer>
  );
}

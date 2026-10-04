import type { ReactNode } from "react";

interface ErrorBannerProps {
  readonly children: ReactNode;
  /** Trailing action (retry/dismiss) — renders on the row's end. */
  readonly action?: ReactNode;
}

export function ErrorBanner({ children, action }: ErrorBannerProps) {
  return (
    <div
      role="alert"
      className="mx-4 my-3 flex items-center gap-3 rounded-xl border border-destructive/40 bg-destructive/15 px-3.5 py-2.5 text-sm text-destructive"
    >
      <span className="min-w-0 flex-1">{children}</span>
      {action}
    </div>
  );
}

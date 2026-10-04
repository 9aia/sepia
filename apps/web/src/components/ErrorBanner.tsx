import type { ReactNode } from "react";

interface ErrorBannerProps {
  readonly children: ReactNode;
}

export function ErrorBanner({ children }: ErrorBannerProps) {
  return (
    <div
      role="alert"
      className="mx-4 my-3 rounded-xl border border-destructive/40 bg-destructive/15 px-3.5 py-2.5 text-sm text-destructive"
    >
      {children}
    </div>
  );
}

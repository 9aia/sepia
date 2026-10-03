import type { ReactNode } from "react";
import { HeadContent, Outlet, Scripts, createRootRoute } from "@tanstack/react-router";
import "../style.css";

export const Route = createRootRoute({
  head: () => ({
    meta: [
      { charSet: "utf-8" },
      { name: "viewport", content: "width=device-width, initial-scale=1" },
      { title: "sepia" },
    ],
  }),
  component: RootComponent,
  errorComponent: ({ error, reset }) => (
    <div className="error-boundary" role="alert">
      <h1>Something went wrong</h1>
      <p>{error instanceof Error ? error.message : String(error)}</p>
      <button type="button" onClick={() => reset()}>
        Try again
      </button>
    </div>
  ),
  notFoundComponent: () => (
    <div className="error-boundary" role="alert">
      Page not found.
    </div>
  ),
});

function RootComponent() {
  return (
    <RootDocument>
      <Outlet />
    </RootDocument>
  );
}

function RootDocument({ children }: Readonly<{ children: ReactNode }>) {
  return (
    <html lang="en">
      <head>
        <HeadContent />
      </head>
      <body>
        {children}
        <Scripts />
      </body>
    </html>
  );
}

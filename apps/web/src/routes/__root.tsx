import { useEffect, type ReactNode } from "react";
import { useStore } from "@tanstack/react-store";
import { settingsStore } from "../lib/settings";
import { QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "../components/ui/tooltip";
import { HeadContent, Outlet, Scripts, createRootRoute } from "@tanstack/react-router";
import { AlertCircleIcon, FileNotFoundIcon } from "@hugeicons/core-free-icons";
import { EmptyScreen } from "../components/EmptyScreen";
import { Button } from "../components/ui/button";
import { queryClient } from "../hooks/query/queryClient";
import "../style.css";

function RouteError({ error, reset }: { error: unknown; reset: () => void }) {
  useEffect(() => {
    console.error(error);
  }, [error]);
  return (
    <main className="flex min-h-svh">
      <EmptyScreen
        className="m-auto max-w-xl"
        icon={AlertCircleIcon}
        title="Something went wrong"
        description="An unexpected error occurred. The details are in the console — try again or go back."
      >
        <Button onClick={() => reset()}>Try again</Button>
      </EmptyScreen>
    </main>
  );
}

export const Route = createRootRoute({
  head: () => ({
    meta: [
      { charSet: "utf-8" },
      { name: "viewport", content: "width=device-width, initial-scale=1" },
      { title: "sepia" },
      { name: "theme-color", content: "#0a0a0a" },
    ],
    links: [{ rel: "manifest", href: "/manifest.webmanifest" }],
  }),
  component: RootComponent,
  errorComponent: (props) => <RouteError {...props} />,
  notFoundComponent: () => (
    <main className="flex min-h-svh">
      <EmptyScreen
        className="m-auto max-w-xl"
        icon={FileNotFoundIcon}
        title="404 — page not found"
        description="The page you were looking for doesn't exist."
      >
        <Button render={<a href="/" />}>Back to sessions</Button>
      </EmptyScreen>
    </main>
  ),
});

function RootComponent() {
  const theme = useStore(settingsStore, (state) => state.theme);
  useEffect(() => {
    // PWA service worker — delivers push notifications.
    if ("serviceWorker" in navigator) {
      void navigator.serviceWorker.register("/sw.js").catch(() => {});
    }
  }, []);

  // Theme — "system" follows the OS; toggles a .light class on <html>.
  useEffect(() => {
    const media = matchMedia("(prefers-color-scheme: light)");
    const apply = (): void => {
      document.documentElement.classList.toggle(
        "light",
        theme === "light" || (theme === "system" && media.matches),
      );
    };
    apply();
    media.addEventListener("change", apply);
    return () => media.removeEventListener("change", apply);
  }, [theme]);
  return (
    <RootDocument>
      <QueryClientProvider client={queryClient}>
        <TooltipProvider delay={300}>
          <Outlet />
        </TooltipProvider>
      </QueryClientProvider>
    </RootDocument>
  );
}

function RootDocument({ children }: Readonly<{ children: ReactNode }>) {
  return (
    <html lang="en" className="dark">
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

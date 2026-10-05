// @vitest-environment happy-dom
/** Leaf-component smoke tests — render to real DOM under happy-dom. */
import { describe, expect, it, vi } from "vite-plus/test";

vi.mock("@hugeicons/react", () => ({
  HugeiconsIcon: () => <svg data-testid="icon" />,
}));

import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { ErrorBanner } from "../components/ErrorBanner";
import { ChatSkeleton } from "../components/ChatSkeleton";
import { EmptyScreen } from "../components/EmptyScreen";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const render = (node: ReactNode): { container: HTMLElement; root: Root } => {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  act(() => {
    root.render(<QueryClientProvider client={client}>{node}</QueryClientProvider>);
  });
  return { container, root };
};

describe("leaf components", () => {
  it("ErrorBanner renders children and the action slot", () => {
    const { container } = render(
      <ErrorBanner action={<button>Retry</button>}>Something broke</ErrorBanner>,
    );
    expect(container.querySelector("[role=alert]")?.textContent).toContain("Something broke");
    expect(container.querySelector("button")?.textContent).toBe("Retry");
  });

  it("ChatSkeleton renders the loading frame", () => {
    const { container } = render(<ChatSkeleton />);
    expect(container.querySelector("[aria-busy]")).not.toBeNull();
    expect(container.querySelectorAll(".animate-pulse, [class*=skeleton]").length).toBeGreaterThan(
      0,
    );
  });

  it("EmptyScreen renders title, description, icon and children slots", () => {
    const { container } = render(
      <EmptyScreen icon={[] as never} title="Nothing here" description="Create one to begin">
        <button>New</button>
      </EmptyScreen>,
    );
    expect(container.textContent).toContain("Nothing here");
    expect(container.textContent).toContain("Create one to begin");
    expect(container.querySelector("button")?.textContent).toBe("New");
    expect(container.querySelector("[data-testid=icon]")).not.toBeNull();
  });
});

describe("marker + reasoning components", () => {
  it("Marker renders separator/border variants", async () => {
    const { Marker, MarkerContent } = await import("../components/marker");
    const { container } = render(
      <>
        <Marker variant="separator">mid</Marker>
        <Marker variant="border">edge</Marker>
        <Marker>
          <MarkerContent>body</MarkerContent>
        </Marker>
      </>,
    );
    const markers = container.querySelectorAll("[data-slot=marker]");
    expect(markers.length).toBe(3);
    expect(markers[0]!.querySelectorAll("[aria-hidden]").length).toBe(2);
    expect(container.textContent).toContain("body");
  });

  it("ReasoningBlock expands while streaming and collapses when done", async () => {
    const { ReasoningBlock } = await import("../components/reasoning-block");
    const { container } = render(<ReasoningBlock done={false} content="thinking…" />);
    expect(container.textContent).toContain("Thinking");
    const { container: done } = render(<ReasoningBlock done content="finished text" />);
    expect(done.textContent).toContain("Reasoning");
  });
});

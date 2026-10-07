// @vitest-environment happy-dom
/**
 * Settings → Desktop (DesktopSection) — the "Node" rename and the
 * no-nodes-connected empty card that mirrors the catalog sections.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vite-plus/test";

vi.mock("@hugeicons/react", () => ({
  HugeiconsIcon: () => <svg data-testid="icon" />,
}));

import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { QueryClientProvider } from "@tanstack/react-query";
import { makeQueryClient, stubFetch, waitFor } from "../test-utils/render-hook";
import { nodesStore } from "../lib/nodes";
import type { NodeDescriptor } from "../lib/types";
import { setSettings } from "../lib/settings";
import { DesktopSection } from "../components/settings/DesktopSection";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const SELF = {
  id: "node-local",
  name: "this machine",
  version: "0",
  agents: ["devin"],
} as NodeDescriptor;

const mounted: Array<{ root: Root; container: HTMLElement }> = [];

const render = (node: ReactNode): { container: HTMLElement; root: Root } => {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  act(() => {
    root.render(<QueryClientProvider client={makeQueryClient()}>{node}</QueryClientProvider>);
  });
  mounted.push({ root, container });
  return { container, root };
};

beforeEach(() => {
  setSettings({
    desktop: { node: null, agent: null, model: null, cwd: null },
    localNodeEnabled: true,
  });
});

afterEach(() => {
  for (const { root, container } of mounted.splice(0)) {
    act(() => root.unmount());
    container.remove();
  }
  setSettings({
    desktop: { node: null, agent: null, model: null, cwd: null },
    localNodeEnabled: true,
  });
  nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [] }));
  vi.unstubAllGlobals();
});

describe("DesktopSection", () => {
  it("labels the environment's first row 'Node'", async () => {
    stubFetch([
      { match: "/api/node", body: SELF },
      { match: "/api/agents", body: { agents: [{ id: "devin", label: "Devin" }] } },
    ]);
    nodesStore.setState(() => ({ self: SELF, selfStatus: "online", peers: [] }));
    const { container } = render(<DesktopSection scrollTo={() => {}} />);

    const section = container.querySelector("[data-spy='desktop']")!;
    expect(section.querySelector("label[for='desktop-node']")?.textContent).toBe("Node");
    expect(section.querySelector("#desktop-node")?.getAttribute("aria-label")).toBe("Desktop node");
    expect(section.textContent).not.toContain("Machine");
    // Descriptions name the picked node, not a hardcoded "local".
    await waitFor(() => section.textContent?.includes("session on this machine") === true);
    expect(section.textContent).toContain("A path on this machine");
  });

  it("with nothing reachable the picks hide behind a 'No nodes connected' card", async () => {
    // Every probe fails — the self descriptor refetch also lands offline.
    stubFetch([{ match: "/api/node", status: 500, body: {} }]);
    nodesStore.setState(() => ({ self: null, selfStatus: "offline", peers: [] }));
    const scrollTo = vi.fn();
    const { container } = render(<DesktopSection scrollTo={scrollTo} />);

    const section = container.querySelector("[data-spy='desktop']")!;
    await waitFor(() => section.textContent?.includes("No nodes connected") === true);
    expect(section.querySelector("#desktop-node")).toBeNull();

    const connect = [...section.querySelectorAll("button")].find(
      (b) => b.textContent === "Add a node",
    );
    expect(connect).not.toBeUndefined();
    act(() => {
      connect!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(scrollTo).toHaveBeenCalledWith("nodes");
  });
});

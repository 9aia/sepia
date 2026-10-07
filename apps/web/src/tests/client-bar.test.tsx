// @vitest-environment happy-dom
/**
 * The sidebar footer's desktop menu (ClientBar) — Node/Agent/Model/
 * Directory rows plus the "No nodes connected" degradation. Menus open on
 * `mousedown` (base-ui) and submenus on `mouseover`/`mousemove` past their
 * rest delay, so tests dispatch the same DOM events against the portal.
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
import { clientStore } from "../lib/client";
import { setSettings, settingsStore } from "../lib/settings";
import { ClientBar } from "../components/session-list/ClientBar";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const SELF: NodeDescriptor = {
  id: "node-local",
  name: "this machine",
  version: "0",
  agents: ["devin", "cline"],
} as NodeDescriptor;

const AGENTS = {
  agents: [
    { id: "devin", label: "Devin" },
    { id: "cline", label: "Cline" },
  ],
};

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

const unmountAll = (): void => {
  for (const { root, container } of mounted.splice(0)) {
    act(() => root.unmount());
    container.remove();
  }
  // Portaled menus live outside the container — clear any stragglers.
  document.querySelectorAll("[data-slot^='dropdown-menu']").forEach((el) => el.remove());
};

const fire = (el: Element, type: string): void => {
  act(() => {
    el.dispatchEvent(new MouseEvent(type, { bubbles: true }));
  });
};

const openMenu = async (): Promise<HTMLElement> => {
  const trigger = document.querySelector("[aria-label='Client menu']");
  expect(trigger).not.toBeNull();
  fire(trigger!, "mousedown");
  await waitFor(() => document.querySelector("[data-slot='dropdown-menu-content']") !== null);
  return document.querySelector("[data-slot='dropdown-menu-content']") as HTMLElement;
};

const openSubmenu = async (menu: HTMLElement, label: string): Promise<HTMLElement> => {
  const trigger = [...menu.querySelectorAll("[data-slot='dropdown-menu-sub-trigger']")].find((t) =>
    t.textContent?.includes(label),
  );
  expect(trigger, `${label} sub-trigger`).not.toBeUndefined();
  fire(trigger!, "mouseover");
  fire(trigger!, "mousemove");
  await waitFor(
    () => document.querySelector("[data-slot='dropdown-menu-sub-content']") !== null,
    3_000,
  );
  const contents = [...document.querySelectorAll("[data-slot='dropdown-menu-sub-content']")];
  const content = contents.find((c) => c.checkVisibility() || true) ?? contents[0];
  return content as HTMLElement;
};

const resetStores = (): void => {
  clientStore.setState(() => ({
    client: {
      id: "client_t",
      label: "Test",
      publicKey: "pk",
      secretKey: "sk",
      algorithm: "none",
    },
  }));
  setSettings({
    desktop: { node: null, agent: null, model: null, cwd: null },
    localNodeEnabled: true,
    disabledAgents: [],
    disabledModels: [],
    models: {},
  });
};

beforeEach(resetStores);
afterEach(() => {
  unmountAll();
  resetStores();
  nodesStore.setState(() => ({ self: null, selfStatus: "unknown", peers: [] }));
  vi.unstubAllGlobals();
});

describe("ClientBar desktop menu", () => {
  it("lists Node / Agent / Model / Directory rows — no 'Machine' anywhere", async () => {
    stubFetch([
      { match: "/api/agents", body: AGENTS },
      { match: "/api/node", body: SELF },
      { match: "/api/sessions", body: { sessions: [] } },
    ]);
    nodesStore.setState(() => ({ self: SELF, selfStatus: "online", peers: [] }));
    render(<ClientBar />);
    const menu = await openMenu();

    expect(menu.textContent).not.toContain("Machine");
    const order = ["Node", "Agent", "Model", "Directory"];
    const positions = order.map((label) => menu.textContent?.indexOf(label) ?? -1);
    expect(positions.every((p) => p >= 0)).toBe(true);
    expect([...positions].sort((a, b) => a - b)).toEqual(positions);
    // The Agent row shows the pick — unset reads "Node default".
    const agentRow = [...menu.querySelectorAll("[data-slot='dropdown-menu-sub-trigger']")].find(
      (t) => t.textContent?.includes("Agent"),
    );
    expect(agentRow?.textContent).toContain("Node default");
  });

  it("the Agent submenu lists the focused node's roster plus 'Node default'", async () => {
    stubFetch([
      { match: "/api/agents", body: AGENTS },
      { match: "/api/node", body: SELF },
      { match: "/api/sessions", body: { sessions: [] } },
    ]);
    nodesStore.setState(() => ({ self: SELF, selfStatus: "online", peers: [] }));
    render(<ClientBar />);
    const menu = await openMenu();

    const submenu = await openSubmenu(menu, "Agent");
    expect(submenu.textContent).toContain("Node default");
    expect(submenu.textContent).toContain("Devin");
    expect(submenu.textContent).toContain("Cline");

    // Picking an agent writes the desktop pick scoped to the focused node.
    const item = [...submenu.querySelectorAll("[data-slot='dropdown-menu-item']")].find((i) =>
      i.textContent?.includes("Cline"),
    );
    expect(item).not.toBeUndefined();
    fire(item!, "mouseup");
    fire(item!, "click");
    await waitFor(() => settingsStore.state.desktop.agent === "cline");
    expect(settingsStore.state.desktop.node).toBeNull();
  });

  it("with nothing reachable the summary and Node row read 'No nodes connected'", async () => {
    // Every fetch fails — local leg down, no peers registered.
    stubFetch([{ match: "/api/sessions", status: 500, body: {} }]);
    nodesStore.setState(() => ({ self: null, selfStatus: "offline", peers: [] }));
    const { container } = render(<ClientBar />);
    await waitFor(() => container.textContent?.includes("No nodes connected") === true);
    expect(container.querySelector("[role=status]")?.getAttribute("aria-label")).toBe(
      "No nodes connected",
    );

    const menu = await openMenu();
    const nodeRow = [...menu.querySelectorAll("[data-slot='dropdown-menu-sub-trigger']")].find(
      (t) => t.textContent?.includes("Node"),
    );
    expect(nodeRow?.textContent).toContain("No nodes connected");

    // No node can answer, so the roster is empty — the Agent submenu says so.
    const submenu = await openSubmenu(menu, "Agent");
    expect(submenu.textContent).toContain("No agents");
    expect(submenu.textContent).toContain("Node default");
  });
});

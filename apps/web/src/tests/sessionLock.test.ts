// @vitest-environment happy-dom
import { afterEach, describe, expect, it } from "vite-plus/test";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { LockMark, lockTooltip } from "../components/session-list/LockMark";
import type { SessionSummary } from "../lib/types";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

type LockState = Pick<SessionSummary, "locked" | "lockHolderPid">;

const free: LockState = { locked: false, lockHolderPid: null };

describe("lockTooltip", () => {
  it("is null for a free session", () => {
    expect(lockTooltip(free)).toBeNull();
  });

  it("names the holder pid when the probe reported one", () => {
    expect(lockTooltip({ locked: true, lockHolderPid: 4242 })).toBe(
      "Held by another process (PID 4242)",
    );
  });

  it("omits the pid when the probe couldn't name it", () => {
    expect(lockTooltip({ locked: true, lockHolderPid: null })).toBe("Held by another process");
  });
});

describe("LockMark", () => {
  const roots: Root[] = [];
  const hosts: HTMLElement[] = [];

  const render = (session: LockState): HTMLElement => {
    const host = document.createElement("div");
    document.body.appendChild(host);
    hosts.push(host);
    const root = createRoot(host);
    roots.push(root);
    act(() => root.render(createElement(LockMark, { session })));
    return host;
  };

  afterEach(() => {
    for (const root of roots.splice(0)) act(() => root.unmount());
    for (const el of hosts.splice(0)) el.remove();
  });

  it("renders nothing while the session is free", () => {
    expect(render(free).childElementCount).toBe(0);
  });

  it("renders a muted, titled mark while held", () => {
    const mark = render({ locked: true, lockHolderPid: 7 }).querySelector("[title]");
    expect(mark?.getAttribute("title")).toBe("Held by another process (PID 7)");
    expect(mark?.getAttribute("aria-label")).toBe("Held by another process (PID 7)");
    expect(mark?.className).toContain("text-muted-foreground");
    expect(mark?.querySelector("svg")).not.toBeNull();
  });
});

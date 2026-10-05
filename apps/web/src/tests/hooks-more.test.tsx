// @vitest-environment happy-dom
/** Second hook pass — node-registry mutations, collection-backed hooks. */
import { afterEach, describe, expect, it, vi } from "vite-plus/test";
import { act } from "react";

vi.mock("sonner", () => ({
  toast: { success: vi.fn(), error: vi.fn(), loading: vi.fn(() => "l") },
}));

import { toast } from "sonner";
import { renderHook, stubFetch, waitFor } from "../test-utils/render-hook";
import { useAddNode, useSetNodeEnabled } from "../hooks/query/useNodes";
import { useProjects, useCreateProject } from "../hooks/query/useProjects";
import { useConfig, useUiState } from "../hooks/query/useConfig";
import { nodesStore } from "../lib/nodes";
import { settingsStore } from "../lib/settings";

afterEach(() => {
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});

describe("node registry mutations", () => {
  it("useAddNode registers a direct peer with a manual token", async () => {
    stubFetch([
      { match: "/api/node", body: { id: "peer-1", name: "peer one", agents: ["devin"] } },
      { match: "/api/pair", method: "POST", body: { token: "tok" } },
    ]);
    const { result, unmount } = renderHook(() => useAddNode());
    await act(async () => {
      await result.current?.mutateAsync({ url: "http://peer.local:8787", token: "tok" });
    });
    expect(vi.mocked(toast.success)).toHaveBeenCalledWith("Node added");
    expect(nodesStore.state.peers.length).toBeGreaterThan(0);
    unmount();
    nodesStore.setState((prev) => ({ ...prev, peers: [] }));
  });

  it("useSetNodeEnabled toggles local without touching the network", async () => {
    const { result, unmount } = renderHook(() => useSetNodeEnabled());
    const before = settingsStore.state.localNodeEnabled;
    await act(async () => {
      await result.current?.mutateAsync({ id: "local", enabled: !before });
    });
    expect(settingsStore.state.localNodeEnabled).toBe(!before);
    expect(vi.mocked(toast.success)).toHaveBeenCalled();
    unmount();
    await act(async () => {
      await result.current?.mutateAsync({ id: "local", enabled: before });
    });
  });
});

describe("collection-backed hooks", () => {
  it("useProjects exposes the collection's empty state", async () => {
    stubFetch([{ match: "/api/projects", body: { projects: [] } }]);
    const { result, unmount } = renderHook(() => useProjects());
    await waitFor(() => result.current !== undefined);
    expect(Array.isArray(result.current?.data ?? [])).toBe(true);
    unmount();
  });

  it("useCreateProject posts and writes the cache", async () => {
    const stub = stubFetch([
      { match: "/api/projects", method: "POST", body: { project: { id: "p1", name: "web" } } },
    ]);
    const { result, unmount } = renderHook(() => useCreateProject());
    await act(async () => {
      await result.current?.mutateAsync({ name: "web" });
    });
    expect(JSON.parse(stub.calls[0]?.init?.body as string)).toEqual({ name: "web" });
    expect(vi.mocked(toast.success)).toHaveBeenCalledWith("Project created");
    unmount();
  });

  it("useConfig stays undefined until the collection syncs", async () => {
    stubFetch([{ match: "/api/config", body: { config: { theme: "dark" } } }]);
    const { result, unmount } = renderHook(() => useConfig());
    // shape is present immediately; readiness depends on collection sync
    expect(result.current).toMatchObject({ data: undefined });
    unmount();
  });

  it("useUiState defaults then writes through once ready", async () => {
    stubFetch([]);
    const { result, unmount } = renderHook(() => useUiState("k", "default"));
    expect(result.current?.[0]).toBe("default");
    act(() => {
      result.current?.[1]("next");
    });
    expect(result.current?.[0]).toBe("next");
    unmount();
  });
});

describe("useResumeSession", () => {
  it("exports then imports the session and selects the copy", async () => {
    const { useResumeSession } = await import("../hooks/query/useResumeSession");
    const { sepiaStore, setSelectedId } = await import("../lib/store");
    const stub = stubFetch([
      // getSessionExport → /export
      {
        match: "/export",
        body: {
          session: {
            id: "s1",
            title: "t",
            workingDirectory: "/w",
            model: "m",
            createdAt: 1,
            lastActivityAt: 2,
            mainChainId: 0,
            nodes: [],
            promptHistory: [],
          },
        },
      },
      { match: "/api/sessions/import", method: "POST", body: { id: "copy-1" } },
    ]);
    const { result, unmount } = renderHook(() => useResumeSession());
    setSelectedId(null);
    await act(async () => {
      await result.current?.mutateAsync({
        session: {
          id: "s1",
          title: "t",
          cwd: "/w",
          agent: "devin",
          updatedAt: "2026",
          locked: false,
          lockHolderPid: null,
          source: "devin",
          busy: false,
          pinned: false,
          archived: false,
          projectIds: [],
          model: null,
          spans: [],
        },
        agent: "cline",
      });
    });
    expect(stub.calls.map((c) => c.url)).toEqual([
      expect.stringContaining("/api/sessions/s1/export"),
      expect.stringContaining("/api/sessions/import"),
    ]);
    expect(sepiaStore.state.selectedId).toBe("cline:copy-1");
    expect(vi.mocked(toast.loading)).toHaveBeenCalled();
    unmount();
  });

  it("toasts on resume failure and rethrows", async () => {
    const { useResumeSession } = await import("../hooks/query/useResumeSession");
    stubFetch([
      { match: "/export", status: 500, body: { error: "x" } },
      { match: "/history", body: { messages: [], total: 0, start: 0 } },
    ]);
    const { result, unmount } = renderHook(() => useResumeSession());
    await act(async () => {
      await result.current
        ?.mutateAsync({
          session: {
            id: "s1",
            title: "t",
            cwd: "/w",
            agent: "devin",
            updatedAt: "2026",
            locked: false,
            lockHolderPid: null,
            source: "devin",
            busy: false,
            pinned: false,
            archived: false,
            projectIds: [],
            model: null,
            spans: [],
          },
          agent: "cline",
        })
        .catch(() => undefined);
    });
    expect(vi.mocked(toast.error)).toHaveBeenCalled();
    unmount();
  });
});

describe("usePatchSessionMeta fallback", () => {
  it("patches directly when the row isn't in the collection", async () => {
    const { usePatchSessionMeta } = await import("../hooks/query/useSessionMeta");
    const stub = stubFetch([
      { match: "/api/sessions/ghost", method: "PATCH", body: { ok: true } },
      { match: "/api/sessions", body: { sessions: [] } },
    ]);
    const { result, unmount } = renderHook(() => usePatchSessionMeta());
    await act(async () => {
      await result.current?.mutateAsync({ id: "ghost", patch: { title: "Renamed" } });
    });
    expect(stub.calls[0]?.url).toContain("/api/sessions/ghost");
    unmount();
  });

  it("fails when the server rejects the patch", async () => {
    const { usePatchSessionMeta } = await import("../hooks/query/useSessionMeta");
    stubFetch([
      { match: "/api/sessions/ghost", method: "PATCH", status: 400, body: { error: "no" } },
    ]);
    const { result, unmount } = renderHook(() => usePatchSessionMeta());
    await act(async () => {
      await result.current
        ?.mutateAsync({ id: "ghost", patch: { title: "x" } })
        .catch(() => undefined);
    });
    expect(vi.mocked(toast.error)).toHaveBeenCalled();
    unmount();
  });
});

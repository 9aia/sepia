// @vitest-environment happy-dom
/**
 * Hook coverage for src/hooks/query — fetch-backed hooks run against a
 * stubbed global fetch; store-backed hooks read the real settings/nodes
 * stores; mutations assert the api call and the UI side effects (toasts,
 * selection, invalidation).
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vite-plus/test";
import { act } from "react";

vi.mock("sonner", () => ({
  toast: {
    success: vi.fn(),
    error: vi.fn(),
    loading: vi.fn(() => "loading-id"),
  },
}));

import { toast } from "sonner";
import { renderHook, stubFetch, waitFor } from "../test-utils/render-hook";
import { useAgents } from "../hooks/query/useAgents";
import { useHealth } from "../hooks/query/useHealth";
import { useDirs } from "../hooks/query/useDirs";
import { useUserInfo } from "../hooks/query/useUserInfo";
import { useSessions } from "../hooks/query/useSessions";
import { useServers } from "../hooks/query/useServers";
import {
  useNodes,
  useMultiNode,
  useNodeLabel,
  useSelfNode,
  useNodeStatuses,
  useNodesConnected,
  usePeerDescriptors,
} from "../hooks/query/useNodes";
import { useAttachSession } from "../hooks/query/useAttachSession";
import { useRespondToPermission } from "../hooks/query/useRespondToPermission";
import { useRenameSession } from "../hooks/query/useRenameSession";
import { useCreateSession } from "../hooks/query/useCreateSession";
import { useDeleteSession } from "../hooks/query/useDeleteSession";
import { useHistory, flattenHistory } from "../hooks/query/useHistory";
import { useCheckpoints, useRestoreSession, restoreSummary } from "../hooks/query/useRestore";
import { useRewindSession } from "../hooks/query/useRewind";
import { nodesStore } from "../lib/nodes";
import { sepiaStore, setSelectedId } from "../lib/store";
import { AuthError } from "../lib/api";

afterEach(() => {
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});

const NODE = { id: "node-local", name: "this machine", version: "0.0.0", agents: ["devin"] };

describe("useHealth", () => {
  it("resolves ok and errors on non-2xx", async () => {
    stubFetch([{ match: "/api/health", body: { status: "ok" } }]);
    const { result, unmount } = renderHook(() => useHealth());
    await waitFor(() => result.current?.isSuccess === true);
    expect(result.current?.data).toEqual({ status: "ok" });
    unmount();
  });

  it("marks the server down on failure", async () => {
    stubFetch([{ match: "/api/health", status: 500, body: {} }]);
    const { result, unmount } = renderHook(() => useHealth());
    await waitFor(() => result.current?.isError === true, 5_000);
    unmount();
  });
});

describe("useDirs", () => {
  it("is disabled for a null path and lists dirs otherwise", async () => {
    const stub = stubFetch([{ match: "/api/fs", body: { dirs: ["/a", "/b"] } }]);
    const off = renderHook(() => useDirs(null));
    expect(off.result.current?.fetchStatus).toBe("idle");
    off.unmount();

    const on = renderHook(() => useDirs("/root"));
    await waitFor(() => on.result.current?.isSuccess === true);
    expect(on.result.current?.data).toEqual(["/a", "/b"]);
    expect(stub.calls[0]?.url).toContain("path=");
    on.unmount();
  });
});

describe("useUserInfo", () => {
  it("swallows non-auth failures but rethrows AuthError", async () => {
    stubFetch([{ match: "/api/user", status: 500, body: { error: "boom" } }]);
    const first = renderHook(() => useUserInfo());
    // the hook swallows non-auth failures → queryFn returns undefined, which
    // react-query flags as an error — just not an AuthError
    await waitFor(() => first.result.current?.isError === true);
    expect(first.result.current?.error).not.toBeInstanceOf(AuthError);
    first.unmount();

    stubFetch([{ match: "/api/user", status: 401, body: {} }]);
    const second = renderHook(() => useUserInfo());
    await waitFor(() => second.result.current?.isError === true);
    expect(second.result.current?.error).toBeInstanceOf(AuthError);
    second.unmount();
  });

  it("returns the profile on success", async () => {
    stubFetch([{ match: "/api/user", body: { user: { username: "u", homedir: "/u" } } }]);
    const { result, unmount } = renderHook(() => useUserInfo());
    await waitFor(() => result.current?.isSuccess === true);
    expect(result.current?.data?.username).toBe("u");
    unmount();
  });
});

describe("useAgents / useSelfNode", () => {
  it("lists agents and resolves the node descriptor", async () => {
    stubFetch([
      { match: "/api/agents", body: { agents: [{ id: "devin", label: "Devin" }] } },
      { match: "/api/node", body: NODE },
    ]);
    const agents = renderHook(() => useAgents());
    await waitFor(() => agents.result.current?.isSuccess === true);
    expect(agents.result.current?.data?.[0]?.id).toBe("devin");
    agents.unmount();

    const self = renderHook(() => useSelfNode());
    await waitFor(() => self.result.current?.isSuccess === true);
    expect(nodesStore.state.self?.id).toBe("node-local");
    expect(nodesStore.state.selfStatus).toBe("online");
    self.unmount();
  });

  it("marks the node offline when the descriptor probe fails non-auth", async () => {
    stubFetch([{ match: "/api/node", status: 500, body: {} }]);
    const self = renderHook(() => useSelfNode());
    await waitFor(() => self.result.current?.isError === true, 5_000);
    expect(nodesStore.state.selfStatus).toBe("offline");
    self.unmount();
    nodesStore.setState((prev) => ({ ...prev, selfStatus: "unknown" }));
  });
});

describe("node store hooks", () => {
  it("useNodes/useMultiNode/useNodeLabel read the registry", () => {
    const { result, unmount } = renderHook(() => ({
      nodes: useNodes(),
      multi: useMultiNode(),
      label: useNodeLabel(undefined),
    }));
    expect(result.current?.nodes.peers).toEqual([]);
    expect(result.current?.multi).toBe(false);
    expect(typeof result.current?.label).toBe("string");
    unmount();
  });

  it("useNodesConnected reports connected/checking/disconnected", async () => {
    const { result, unmount } = renderHook(() => useNodesConnected());
    await waitFor(() => result.current !== undefined);
    // local enabled + selfStatus != offline → connected
    expect(result.current).toBe("connected");
    unmount();
  });

  it("useNodeStatuses/usePeerDescriptors skip disabled peers", async () => {
    stubFetch([]);
    const peers = [
      { id: "p1", name: "p1", enabled: false, via: "direct" as const, url: "http://x" },
    ] as never;
    const { result, unmount } = renderHook(() => ({
      statuses: useNodeStatuses(peers),
      descriptors: usePeerDescriptors(peers),
    }));
    expect(result.current?.statuses).toEqual([undefined]);
    expect(result.current?.descriptors).toEqual([undefined]);
    unmount();
  });
});

describe("mutations", () => {
  beforeEach(() => {
    setSelectedId(null);
  });

  it("useAttachSession posts the attach body", async () => {
    const stub = stubFetch([
      {
        match: "/api/sessions/s1/attach",
        method: "POST",
        body: { attached: true, readOnly: false, agentId: "devin", capabilities: {} },
      },
    ]);
    const { result, unmount } = renderHook(() => useAttachSession());
    await act(async () => {
      await result.current?.mutateAsync({ id: "s1", takeover: true, model: "m" });
    });
    const body = JSON.parse(stub.calls[0]?.init?.body as string);
    expect(body).toMatchObject({ takeover: true, model: "m" });
    unmount();
  });

  it("useRespondToPermission posts the decision", async () => {
    const stub = stubFetch([{ match: "/permission", method: "POST", body: { ok: true } }]);
    const { result, unmount } = renderHook(() => useRespondToPermission());
    await act(async () => {
      await result.current?.mutateAsync({ sessionId: "s1", requestId: "r1", optionId: "allow" });
    });
    expect(JSON.parse(stub.calls[0]?.init?.body as string)).toEqual({
      requestId: "r1",
      optionId: "allow",
    });
    unmount();
  });

  it("useRenameSession patches and invalidates", async () => {
    const stub = stubFetch([{ match: "/api/sessions/s9", method: "PATCH", body: { ok: true } }]);
    const { result, queryClient, unmount } = renderHook(() => useRenameSession());
    const spy = vi.spyOn(queryClient, "invalidateQueries");
    await act(async () => {
      await result.current?.mutateAsync({ id: "s9", title: "New" });
    });
    expect(JSON.parse(stub.calls[0]?.init?.body as string)).toMatchObject({ title: "New" });
    expect(spy).toHaveBeenCalled();
    unmount();
  });

  it("useCreateSession selects the created id", async () => {
    stubFetch([
      { match: "/api/sessions", method: "POST", body: { id: "new-1", agentId: "devin" } },
    ]);
    const { result, unmount } = renderHook(() => useCreateSession());
    await act(async () => {
      await result.current?.mutateAsync({ cwd: "/w" });
    });
    expect(sepiaStore.state.selectedId).toBe("devin:new-1");
    unmount();
  });

  it("useCreateSession toasts on failure", async () => {
    stubFetch([{ match: "/api/sessions", method: "POST", status: 500, body: { error: "bad" } }]);
    const { result, unmount } = renderHook(() => useCreateSession());
    await act(async () => {
      await result.current?.mutateAsync({ cwd: "/w" }).catch(() => undefined);
    });
    expect(vi.mocked(toast.error)).toHaveBeenCalled();
    unmount();
  });

  it("useDeleteSession deletes an unlisted id directly and clears selection", async () => {
    const stub = stubFetch([
      { match: "/api/sessions/ghost", method: "DELETE", body: { ok: true } },
    ]);
    setSelectedId("ghost");
    const { result, unmount } = renderHook(() => useDeleteSession());
    await act(async () => {
      await result.current?.mutateAsync({ id: "ghost" });
    });
    expect(stub.calls[0]?.init?.method).toBe("DELETE");
    expect(sepiaStore.state.selectedId).toBeNull();
    expect(vi.mocked(toast.success)).toHaveBeenCalled();
    unmount();
  });

  it("useRestoreSession and useRewindSession post confirm bodies", async () => {
    const stub = stubFetch([
      { match: "/restore", method: "POST", body: { restored: [], skipped: [] } },
      { match: "/rewind", method: "POST", body: { kept: 2, removed: 1 } },
    ]);
    const restore = renderHook(() => useRestoreSession());
    await act(async () => {
      await restore.result.current?.mutateAsync({
        sessionId: "s1",
        selector: { path: "/w/a.ts" },
      });
    });
    expect(JSON.parse(stub.calls[0]?.init?.body as string)).toMatchObject({
      confirm: true,
      path: "/w/a.ts",
    });
    restore.unmount();

    const rewind = renderHook(() => useRewindSession());
    await act(async () => {
      await rewind.result.current?.mutateAsync({
        sessionId: "s1",
        selector: { turns: 2 },
      });
    });
    expect(JSON.parse(stub.calls[1]?.init?.body as string)).toMatchObject({
      confirm: true,
      turns: 2,
    });
    rewind.unmount();
  });

  it("useCheckpoints fetches lazily and restoreSummary composes", async () => {
    stubFetch([{ match: "/checkpoints", body: { checkpoints: [{ ref: "abc", createdAt: 1 }] } }]);
    const off = renderHook(() => useCheckpoints(null));
    expect(off.result.current?.fetchStatus).toBe("idle");
    off.unmount();
    const on = renderHook(() => useCheckpoints("s1"));
    await waitFor(() => on.result.current?.isSuccess === true);
    expect(on.result.current?.data?.[0]?.ref).toBe("abc");
    on.unmount();

    expect(restoreSummary({ restored: [], skipped: [] })).toBe("Nothing to restore");
    expect(
      restoreSummary({
        restored: [
          { path: "a", action: "written" },
          { path: "b", action: "unchanged" },
        ],
        skipped: [{ path: "c", reason: "x" }],
      }),
    ).toBe("1 file restored · 1 skipped");
  });
});

describe("useHistory", () => {
  it("flattenHistory reverses pages into chronological order", () => {
    const data = {
      pages: [
        { messages: [{ nodeId: 3 }], start: 0, total: 4 },
        { messages: [{ nodeId: 1 }, { nodeId: 2 }], start: 1, total: 4 },
      ],
      pageParams: [0, 1],
    };
    // newest-first pages reverse to oldest-first rows
    const flat = flattenHistory(data as never);
    expect(flat.map((m) => m.nodeId)).toEqual([1, 2, 3]);
    expect(flattenHistory(undefined)).toEqual([]);
  });

  it("fetches the first page for a live sessionId", async () => {
    const stub = stubFetch([
      {
        match: "/history",
        body: { messages: [{ nodeId: 0, role: "user", content: "hi" }], total: 1, start: 0 },
      },
    ]);
    const { result, unmount } = renderHook(() => useHistory("s1"));
    await waitFor(() => result.current?.isSuccess === true);
    expect(stub.calls[0]?.url).toContain("/api/sessions/s1/history");
    unmount();
  });
});

describe("useSessions / useServers fan-out", () => {
  it("lists merged sessions and servers over the stubbed api", async () => {
    stubFetch([
      {
        match: "/api/sessions",
        body: {
          sessions: [
            {
              id: "s1",
              title: "t",
              cwd: "/w",
              agent: "devin",
              updatedAt: "2026-01-01",
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
          ],
        },
      },
      { match: "/api/servers", body: { servers: [] } },
    ]);
    const sessions = renderHook(() => useSessions());
    await waitFor(() => sessions.result.current?.isSuccess === true);
    const rows = sessions.result.current?.data ?? [];
    expect(rows.some((s: { id: string }) => s.id === "s1")).toBe(true);
    sessions.unmount();

    const servers = renderHook(() => useServers());
    await waitFor(() => servers.result.current?.isSuccess === true);
    expect(servers.result.current?.data).toEqual([]);
    servers.unmount();
  });
});

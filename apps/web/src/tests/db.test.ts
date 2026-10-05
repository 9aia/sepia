import { afterEach, describe, expect, it, vi } from "vite-plus/test";
import { QueryClient } from "@tanstack/react-query";
import { createSessionsCollection } from "../lib/db";
import type { SessionSummary } from "../lib/types";

const makeSession = (over: Partial<SessionSummary>): SessionSummary => ({
  id: "s1",
  title: "Session",
  cwd: "/tmp/work",
  agent: "devin",
  updatedAt: "2024-01-01T00:00:00.000Z",
  locked: false,
  lockHolderPid: null,
  source: "test",
  busy: false,
  pinned: false,
  archived: false,
  projectIds: [],
  model: null,
  spans: [],
  ...over,
});

interface ApiCall {
  method: string;
  url: string;
  body: unknown;
}

/** A tiny in-memory stand-in for the session API: GET/PATCH/DELETE backed by `rows`. */
const installFetch = (rows: SessionSummary[], fail?: { patch?: boolean; del?: boolean }) => {
  const calls: ApiCall[] = [];
  vi.stubGlobal("fetch", async (input: RequestInfo | URL, init?: RequestInit) => {
    const url =
      typeof input === "string" ? input : input instanceof Request ? input.url : input.toString();
    const method = init?.method ?? "GET";
    const body = typeof init?.body === "string" ? (JSON.parse(init.body) as unknown) : undefined;
    calls.push({ method, url, body });
    if (method === "GET" && (url === "/api/sessions" || url.startsWith("/api/sessions?"))) {
      return Response.json({ sessions: rows });
    }
    const match = /^\/api\/sessions\/([^?]+)(?:\?(.*))?$/.exec(url);
    if (match !== null) {
      const id = decodeURIComponent(match[1] ?? "");
      const agent = new URLSearchParams(match[2] ?? "").get("agent") ?? undefined;
      const row = rows.find((r) => r.id === id && (agent === undefined || r.agent === agent));
      if (row === undefined) return new Response("missing", { status: 404 });
      if (method === "PATCH") {
        if (fail?.patch === true) return new Response("nope", { status: 500 });
        Object.assign(row, body);
        return Response.json({ ok: true });
      }
      if (method === "DELETE") {
        if (fail?.del === true) return new Response("nope", { status: 500 });
        rows.splice(rows.indexOf(row), 1);
        return Response.json({ ok: true });
      }
    }
    return new Response("bad", { status: 400 });
  });
  return calls;
};

const newClient = () => new QueryClient({ defaultOptions: { queries: { retry: false } } });

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("sessions collection", () => {
  it("keys rows by agent:id so cross-agent id collisions don't collapse", async () => {
    const rows = [
      makeSession({ id: "dup", agent: "devin", title: "Devin one" }),
      makeSession({ id: "dup", agent: "cline", title: "Cline one" }),
    ];
    installFetch(rows);
    const collection = createSessionsCollection(newClient());
    await collection.preload();
    expect(collection.size).toBe(2);
    expect(collection.get("devin:dup")?.title).toBe("Devin one");
    expect(collection.get("cline:dup")?.title).toBe("Cline one");
  });

  it("pushes refetched snapshots as keyed writes, not duplicates", async () => {
    const rows = [makeSession({ id: "a" }), makeSession({ id: "b", title: "old" })];
    installFetch(rows);
    const client = newClient();
    const collection = createSessionsCollection(client);
    await collection.preload();
    expect(collection.size).toBe(2);
    rows[1] = makeSession({ id: "b", title: "new" });
    // No useQuery observer is mounted in this test, so the query is
    // inactive — type "all" forces the refetch a mounted consumer would get.
    await client.refetchQueries({ queryKey: ["sessions"], type: "all" });
    expect(collection.size).toBe(2);
    expect(collection.get("devin:b")?.title).toBe("new");
  });

  it("applies updates optimistically and PATCHes with the ?agent= scope", async () => {
    const rows = [makeSession({ id: "a" }), makeSession({ id: "a", agent: "cline" })];
    const calls = installFetch(rows);
    const collection = createSessionsCollection(newClient());
    await collection.preload();

    const tx = collection.update("cline:a", (draft) => {
      draft.pinned = true;
    });
    // Optimistic — visible before the server round-trip settles.
    expect(collection.get("cline:a")?.pinned).toBe(true);
    await tx.when("settled");
    expect(collection.get("cline:a")?.pinned).toBe(true);
    // The sibling devin row with the same bare id is untouched.
    expect(collection.get("devin:a")?.pinned).toBe(false);

    const patch = calls.find((call) => call.method === "PATCH");
    expect(patch?.url).toBe("/api/sessions/a?agent=cline");
    expect(patch?.body).toEqual({ pinned: true });
  });

  it("rolls back the optimistic update when the PATCH fails", async () => {
    const rows = [makeSession({ id: "a" })];
    installFetch(rows, { patch: true });
    const collection = createSessionsCollection(newClient());
    await collection.preload();

    const tx = collection.update("devin:a", (draft) => {
      draft.pinned = true;
    });
    expect(collection.get("devin:a")?.pinned).toBe(true);
    await expect(tx.when("settled")).rejects.toThrow("rejected");
    expect(collection.get("devin:a")?.pinned).toBe(false);
  });

  it("deletes optimistically and DELETEs with the ?agent= scope", async () => {
    const rows = [makeSession({ id: "a" }), makeSession({ id: "b" })];
    const calls = installFetch(rows);
    const collection = createSessionsCollection(newClient());
    await collection.preload();

    const tx = collection.delete("devin:a");
    expect(collection.get("devin:a")).toBeUndefined();
    await tx.when("settled");
    expect(collection.get("devin:a")).toBeUndefined();
    expect(collection.size).toBe(1);

    const del = calls.find((call) => call.method === "DELETE");
    expect(del?.url).toBe("/api/sessions/a?agent=devin");
  });

  it("restores the row when the DELETE fails", async () => {
    const rows = [makeSession({ id: "a" })];
    installFetch(rows, { del: true });
    const collection = createSessionsCollection(newClient());
    await collection.preload();

    const tx = collection.delete("devin:a");
    expect(collection.get("devin:a")).toBeUndefined();
    await expect(tx.when("settled")).rejects.toThrow();
    expect(collection.get("devin:a")?.id).toBe("a");
  });
});

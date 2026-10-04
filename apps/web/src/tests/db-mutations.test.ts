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
  ...over,
});

interface ApiCall {
  method: string;
  url: string;
  body: unknown;
}

const installFetch = (rows: SessionSummary[]) => {
  const calls: ApiCall[] = [];
  vi.stubGlobal("fetch", async (input: RequestInfo | URL, init?: RequestInit) => {
    const url =
      typeof input === "string" ? input : input instanceof Request ? input.url : input.toString();
    const method = init?.method ?? "GET";
    const body = typeof init?.body === "string" ? (JSON.parse(init.body) as unknown) : undefined;
    calls.push({ method, url, body });
    if (method === "GET" && url === "/api/sessions") {
      return Response.json({ sessions: rows });
    }
    return Response.json({ ok: true });
  });
  return calls;
};

const newClient = () => new QueryClient({ defaultOptions: { queries: { retry: false } } });

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("sessions collection — meta patch shape", () => {
  it("maps every mutable field into the PATCH body, stripping node prefixes", async () => {
    const rows = [makeSession({ id: "a" })];
    const calls = installFetch(rows);
    const collection = createSessionsCollection(newClient());
    await collection.preload();

    const tx = collection.update("devin:a", (draft) => {
      draft.title = "Renamed";
      draft.pinned = true;
      draft.archived = true;
      draft.projectIds = ["local:proj_1", "proj_2"];
      draft.model = "claude-x";
    });
    await tx.when("settled");

    const patch = calls.find((call) => call.method === "PATCH");
    // Node-namespaced project refs collapse to the bare ids the meta overlay stores.
    expect(patch?.body).toEqual({
      title: "Renamed",
      pinned: true,
      archived: true,
      projectIds: ["proj_1", "proj_2"],
      model: "claude-x",
    });
  });

  it("omits untouched fields from the PATCH body — a model clear sends null", async () => {
    const calls = installFetch([makeSession({ id: "a", title: "Keep", model: "claude-x" })]);
    const collection = createSessionsCollection(newClient());
    await collection.preload();

    const tx = collection.update("devin:a", (draft) => {
      draft.model = null;
    });
    await tx.when("settled");

    const patch = calls.find((call) => call.method === "PATCH");
    expect(patch?.body).toEqual({ model: null });
  });
});

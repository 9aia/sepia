/**
 * Project transfer routes — NDJSON bundle export, streamed import, and the
 * pull/push verbs that chain them node-to-node (the peer fetch rides the
 * `fetchImpl` seam so no real network is involved).
 */
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, describe, expect, it } from "vite-plus/test";
import { Effect } from "effect";
import { MessageNode, Session } from "sepia-core";
import { Conversion } from "sepia-convert";
import type { ControlPlaneService } from "sepia-session-control";
import { createApp } from "../src/app";
import { createMetaStore, type MetaStore } from "../src/meta";

const tmp = mkdtempSync(join(tmpdir(), "sepia-transfer-"));
afterAll(() => rmSync(tmp, { recursive: true, force: true }));

let metaCounter = 0;
const freshMeta = (): MetaStore => createMetaStore(join(tmp, `meta-${(metaCounter += 1)}.json`));

const SESSION_IR = Session.make({
  id: "sess-a",
  title: "Source session",
  workingDirectory: "/work/a",
  model: "test-model",
  createdAt: 1_700_000_000,
  lastActivityAt: 1_700_000_100,
  mainChainId: 1,
  metadata: null,
  nodes: [
    MessageNode.make({
      nodeId: 0,
      role: "user",
      content: "hello",
      createdAt: 1_700_000_000,
      metadata: null,
    }),
    MessageNode.make({
      nodeId: 1,
      role: "assistant",
      content: "hi there",
      createdAt: 1_700_000_050,
      metadata: null,
    }),
  ],
  promptHistory: [],
});

const SESSION_IR_2 = Session.make({
  id: "sess-b",
  title: "Second session",
  workingDirectory: "/work/b",
  model: "test-model",
  createdAt: 1_700_000_100,
  lastActivityAt: 1_700_000_200,
  mainChainId: 1,
  metadata: null,
  nodes: [
    MessageNode.make({
      nodeId: 0,
      role: "user",
      content: "again",
      createdAt: 1_700_000_100,
      metadata: null,
    }),
  ],
  promptHistory: [],
});

const summaryOf = (session: Session, agent = "devin") => ({
  id: session.id,
  title: session.title,
  cwd: session.workingDirectory,
  agent,
  updatedAt: new Date(session.lastActivityAt * 1000).toISOString(),
  locked: false,
  lockHolderPid: null,
  source: agent,
  busy: false,
});

const planeOver = (sessions: ReadonlyArray<{ session: Session; agent: string }>) => {
  const byId = new Map(sessions.map((entry) => [entry.session.id, entry]));
  const plane: ControlPlaneService = {
    listSessions: () => Effect.succeed(sessions.map((e) => summaryOf(e.session, e.agent))),
    getHistory: () => Effect.succeed({ messages: [], total: 0, start: 0 }),
    getSession: (id) =>
      byId.has(id)
        ? Effect.succeed(byId.get(id)!.session)
        : Effect.fail(
            Object.assign(new Error(`not found: ${id}`), {
              _tag: "ControlError",
              code: "not_found",
              cause: undefined,
            }) as never,
          ),
    createSession: () => Effect.succeed({ id: "x", agentId: "devin", capabilities: {} as never }),
    attach: () => Effect.succeed({} as never),
    detach: () => Effect.void,
    prompt: () => Effect.void,
    cancel: () => Effect.void,
    deleteSession: () => Effect.void,
    respondToPermission: () => Effect.void,
    restore: () => Effect.succeed({ restored: [], skipped: [] }),
    rewind: () => Effect.succeed({ kept: 0, removed: 0 }),
    subscribe: () => Effect.succeed(() => {}),
    listAgents: () => [],
    closeAll: () => Effect.void,
  };
  return plane;
};

const get = (path: string): Request =>
  new Request(`http://localhost:8787${path}`, {
    headers: { origin: "http://localhost:3000" },
  });

const post = (path: string, body?: unknown): Request =>
  new Request(`http://localhost:8787${path}`, {
    method: "POST",
    headers: { origin: "http://localhost:3000", "content-type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });

const postNdjson = (path: string, body: string | ReadableStream<Uint8Array>): Request =>
  new Request(`http://localhost:8787${path}`, {
    method: "POST",
    headers: { origin: "http://localhost:3000", "content-type": "application/x-ndjson" },
    body,
    ...({ duplex: "half" } as Record<string, unknown>),
  });

/** Read an SSE response into `{event, data}` pairs. */
const readSse = async (
  res: Response,
): Promise<Array<{ event: string; data: Record<string, unknown> }>> => {
  const text = await res.text();
  return text
    .split("\n\n")
    .filter((part) => part.trim() !== "")
    .map((part) => {
      const event = /^event: (.*)$/m.exec(part)?.[1] ?? "";
      const data = /^data: (.*)$/m.exec(part)?.[1] ?? "{}";
      return { event, data: JSON.parse(data) as Record<string, unknown> };
    });
};

const bundleFor = async (meta: MetaStore, projectName = "Proj"): Promise<string> => {
  const plane = planeOver([
    { session: SESSION_IR, agent: "devin" },
    { session: SESSION_IR_2, agent: "cline" },
  ]);
  const app = createApp(plane, { meta });
  const created = await app(post("/api/projects", { name: projectName }));
  const { project } = (await created.json()) as { project: { id: string; name: string } };
  await app(
    new Request("http://localhost:8787/api/sessions/sess-a", {
      method: "PATCH",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ pinned: true, projectIds: [project.id] }),
    }),
  );
  await app(
    new Request("http://localhost:8787/api/sessions/sess-b", {
      method: "PATCH",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ projectIds: [project.id] }),
    }),
  );
  const res = await app(get(`/api/projects/${project.id}/export`));
  expect(res.status).toBe(200);
  expect(res.headers.get("content-type")).toBe("application/x-ndjson");
  return res.text();
};

describe("project export", () => {
  it("GET /api/projects/:id/export streams an NDJSON bundle", async () => {
    const meta = freshMeta();
    const ndjson = await bundleFor(meta, "Alpha");
    const lines = ndjson
      .split("\n")
      .filter((l) => l !== "")
      .map((l) => JSON.parse(l) as Record<string, unknown>);

    expect(lines[0]).toMatchObject({ type: "project", version: 1, name: "Alpha", sessions: 2 });
    const sessions = lines.filter((l) => l.type === "session");
    expect(sessions).toHaveLength(2);
    // Full IR — nodes ride the line verbatim.
    const first = sessions[0]!;
    expect(first.id).toBe("sess-a");
    expect(first.agent).toBe("devin");
    expect(first.title).toBe("Source session");
    expect((first.session as { nodes: unknown[] }).nodes).toHaveLength(2);
    // The meta overlay travels (pin) minus node-local projectIds.
    expect(first.meta).toMatchObject({ pinned: true });
    expect((first.meta as Record<string, unknown>).projectIds).toBeUndefined();
    // A cline member keeps its source agent for store routing.
    expect(sessions[1]?.agent).toBe("cline");
    expect(lines.at(-1)).toMatchObject({ type: "end", sessions: 2, skipped: 0 });
  });

  it("404s for an unknown project and 501s without a meta store", async () => {
    const meta = freshMeta();
    const app = createApp(planeOver([]), { meta });
    expect((await app(get("/api/projects/nope/export"))).status).toBe(404);
    const bare = createApp(planeOver([]));
    expect((await bare(get("/api/projects/x/export"))).status).toBe(501);
  });

  it("records a skipped line for a member whose store read fails", async () => {
    const meta = freshMeta();
    // A summary that lists but can't be read as IR (getSession 404s).
    const plane = planeOver([{ session: SESSION_IR, agent: "devin" }]);
    const ghost = {
      id: "ghost",
      title: "Ghost",
      cwd: "/x",
      agent: "devin",
      updatedAt: new Date(0).toISOString(),
      locked: false,
      lockHolderPid: null,
      source: "devin",
      busy: false,
    };
    const mixed: ControlPlaneService = {
      ...plane,
      listSessions: () => Effect.succeed([summaryOf(SESSION_IR), ghost]),
    };
    const app = createApp(mixed, { meta });
    const { project } = (await (await app(post("/api/projects", { name: "P" }))).json()) as {
      project: { id: string };
    };
    for (const id of ["sess-a", "ghost"]) {
      await app(
        new Request(`http://localhost:8787/api/sessions/${id}`, {
          method: "PATCH",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ projectIds: [project.id] }),
        }),
      );
    }
    const res = await app(get(`/api/projects/${project.id}/export`));
    const lines = (await res.text())
      .split("\n")
      .filter((l) => l !== "")
      .map((l) => JSON.parse(l) as Record<string, unknown>);
    expect(lines.filter((l) => l.type === "session")).toHaveLength(1);
    expect(lines.find((l) => l.type === "skipped")).toMatchObject({ id: "ghost" });
    expect(lines.at(-1)).toMatchObject({ type: "end", sessions: 1, skipped: 1 });
  });
});

describe("project import", () => {
  it("rebuilds sessions through the import executor and re-points projectIds", async () => {
    const sourceMeta = freshMeta();
    const bundle = await bundleFor(sourceMeta, "Alpha");

    const targetMeta = freshMeta();
    const imported: Array<{ id: string; agent: string; nodes: number }> = [];
    const app = createApp(planeOver([]), {
      meta: targetMeta,
      convert: { dbPath: "/unused", clineDir: "/unused" },
      importSession: (session, agent) =>
        Effect.sync(() => {
          imported.push({ id: session.id, agent, nodes: session.nodes.length });
          return `stored-${session.id}`;
        }),
    });

    const res = await app(postNdjson("/api/projects/import", bundle));
    expect(res.status).toBe(201);
    const result = (await res.json()) as {
      project: { id: string; name: string };
      imported: Array<{ id: string; sourceId: string; agent: string }>;
      skipped: unknown[];
      truncated: boolean;
    };
    // The bundle's project id is preserved — a re-pull updates, not clones.
    expect(result.project.name).toBe("Alpha");
    expect(result.imported).toHaveLength(2);
    expect(result.truncated).toBe(false);
    // cline members route to the cline store; everything else to devin.
    expect(imported).toEqual([
      { id: "sess-a", agent: "devin", nodes: 2 },
      { id: "sess-b", agent: "cline", nodes: 1 },
    ]);

    const project = targetMeta.listProjects()[0]!;
    // Meta overlay re-points at the local project + keeps the source's pin.
    const metaA = targetMeta.of("stored-sess-a")!;
    expect(metaA.projectIds).toEqual([project.id]);
    expect(metaA.pinned).toBe(true);
    expect(metaA.agent).toBe("devin");
    expect(metaA.cwd).toBe("/work/a");
    // Provenance span for the local import run.
    expect(metaA.spans?.at(-1)?.agent).toBe("devin");
  });

  it("is idempotent by id — a re-import updates the project, not a clone", async () => {
    const sourceMeta = freshMeta();
    const bundle = await bundleFor(sourceMeta);
    const targetMeta = freshMeta();
    const app = createApp(planeOver([]), {
      meta: targetMeta,
      importSession: (session) => Effect.succeed(session.id),
    });

    const first = await app(postNdjson("/api/projects/import", bundle));
    expect(first.status).toBe(201);
    const again = await app(postNdjson("/api/projects/import", bundle));
    expect(again.status).toBe(201);
    // One project row, source id kept.
    expect(targetMeta.listProjects()).toHaveLength(1);
  });

  it("400s on a non-bundle body and on missing/malformed lines", async () => {
    const app = createApp(planeOver([]), {
      meta: freshMeta(),
      importSession: (session) => Effect.succeed(session.id),
    });
    const empty = await app(postNdjson("/api/projects/import", ""));
    expect(empty.status).toBe(400);
    const bad = await app(postNdjson("/api/projects/import", "not json\n"));
    expect(bad.status).toBe(400);
    const noHeader = await app(
      postNdjson("/api/projects/import", `${JSON.stringify({ type: "end" })}\n`),
    );
    expect(noHeader.status).toBe(400);
  });

  it("skips a session line with malformed IR instead of failing the bundle", async () => {
    const bundle =
      `${JSON.stringify({ type: "project", version: 1, id: "p1", name: "P", sessions: 2 })}\n` +
      `${JSON.stringify({ type: "session", id: "bad", agent: "devin", session: { id: 42 } })}\n` +
      `${JSON.stringify({ type: "session", id: "ok", agent: "devin", session: Conversion.sessionToJson(SESSION_IR) })}\n` +
      `${JSON.stringify({ type: "end", sessions: 2, skipped: 0 })}\n`;
    const app = createApp(planeOver([]), {
      meta: freshMeta(),
      importSession: (session) => Effect.succeed(`stored-${session.id}`),
    });
    const res = await app(postNdjson("/api/projects/import", bundle));
    expect(res.status).toBe(201);
    const result = (await res.json()) as {
      imported: unknown[];
      skipped: Array<{ id: string }>;
    };
    expect(result.imported).toHaveLength(1);
    expect(result.skipped).toEqual([{ id: "bad", error: expect.any(String) }]);
  });

  it("501s when meta or the import executor is missing", async () => {
    const noMeta = createApp(planeOver([]));
    expect((await noMeta(postNdjson("/api/projects/import", "x\n"))).status).toBe(501);
    const noExec = createApp(planeOver([]), { meta: freshMeta() });
    expect((await noExec(postNdjson("/api/projects/import", "x\n"))).status).toBe(501);
  });
});

describe("project pull", () => {
  it("fetches the source's export and imports it, streaming progress", async () => {
    const sourceMeta = freshMeta();
    const bundle = await bundleFor(sourceMeta);

    const seen: string[] = [];
    const fetchImpl = async (
      input: Parameters<typeof fetch>[0],
      init?: Parameters<typeof fetch>[1],
    ): Promise<Response> => {
      const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
      const auth = (init?.headers as Record<string, string> | undefined)?.authorization;
      seen.push(`${url} ${String(auth)}`);
      return new Response(bundle, { status: 200 });
    };
    const targetMeta = freshMeta();
    const app = createApp(planeOver([]), {
      meta: targetMeta,
      fetchImpl: fetchImpl as typeof fetch,
      importSession: (session) => Effect.succeed(session.id),
      node: { id: "node_target", name: "target", version: "0" },
    });

    const res = await app(
      post("/api/projects/pull", {
        source: { url: "http://peer:8787", token: "peer-secret" },
        project: "proj_remote",
      }),
    );
    expect(res.status).toBe(200);
    expect(res.headers.get("content-type")).toContain("text/event-stream");
    const frames = await readSse(res);
    expect(frames[0]).toMatchObject({
      event: "start",
      data: { direction: "pull", source: "http://peer:8787", project: "proj_remote" },
    });
    // The node fetched the peer's export with the supplied credential.
    expect(seen).toEqual(["http://peer:8787/api/projects/proj_remote/export Bearer peer-secret"]);
    const sessions = frames.filter((f) => f.event === "session");
    expect(sessions).toHaveLength(2);
    expect(sessions[0]?.data).toMatchObject({ index: 1, total: 2, sourceId: "sess-a" });
    const done = frames.at(-1);
    expect(done?.event).toBe("done");
    expect(done?.data).toMatchObject({ truncated: false });
    expect((done?.data.imported as unknown[] | undefined)?.length).toBe(2);
    // Sessions land tagged to the pulled project id.
    const project = targetMeta.listProjects()[0]!;
    expect(targetMeta.of("sess-a")?.projectIds).toEqual([project.id]);
  });

  it("emits an error frame when the source refuses", async () => {
    const fetchImpl = async (): Promise<Response> =>
      new Response(JSON.stringify({ error: "Unauthorized" }), { status: 401 });
    const app = createApp(planeOver([]), {
      meta: freshMeta(),
      fetchImpl: fetchImpl as typeof fetch,
      importSession: (session) => Effect.succeed(session.id),
    });
    const res = await app(
      post("/api/projects/pull", {
        source: { url: "http://peer:8787", token: "wrong" },
        project: "p1",
      }),
    );
    const frames = await readSse(res);
    expect(frames.at(-1)?.event).toBe("error");
    expect(String(frames.at(-1)?.data.error)).toContain("Unauthorized");
  });

  it("validates the request body", async () => {
    const app = createApp(planeOver([]), {
      meta: freshMeta(),
      importSession: (session) => Effect.succeed(session.id),
    });
    for (const body of [
      {},
      { source: { url: "notaurl" }, project: "p" },
      { source: { url: "ftp://x" }, project: "p" },
      { source: { url: "http://x" } },
      { source: { url: "http://x", token: 4 }, project: "p" },
    ]) {
      expect((await app(post("/api/projects/pull", body))).status).toBe(400);
    }
  });
});

describe("project push", () => {
  it("posts the local bundle to the target's import endpoint", async () => {
    const meta = freshMeta();
    const plane = planeOver([{ session: SESSION_IR, agent: "devin" }]);
    const app = createApp(plane, { meta });
    const { project } = (await (await app(post("/api/projects", { name: "Shipped" }))).json()) as {
      project: { id: string };
    };
    await app(
      new Request("http://localhost:8787/api/sessions/sess-a", {
        method: "PATCH",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ projectIds: [project.id] }),
      }),
    );

    const captured: { url: string; auth: string | undefined; body: string }[] = [];
    const fetchImpl = async (
      input: Parameters<typeof fetch>[0],
      init?: Parameters<typeof fetch>[1],
    ): Promise<Response> => {
      const body = await new Response(
        init?.body as ConstructorParameters<typeof Response>[0],
      ).text();
      captured.push({
        url: typeof input === "string" ? input : input instanceof URL ? input.href : input.url,
        auth: (init?.headers as Record<string, string> | undefined)?.authorization,
        body,
      });
      return new Response(JSON.stringify({ project, imported: [], skipped: [] }), {
        status: 201,
        headers: { "content-type": "application/json" },
      });
    };
    const pushing = createApp(plane, {
      meta,
      fetchImpl: fetchImpl as typeof fetch,
    });
    const res = await pushing(
      post(`/api/projects/${project.id}/push`, {
        target: { url: "http://peer-b:8787/", token: "t-secret" },
      }),
    );
    const frames = await readSse(res);
    expect(frames[0]?.event).toBe("start");
    const done = frames.at(-1);
    expect(done?.event).toBe("done");
    expect(done?.data).toMatchObject({ target: "http://peer-b:8787" });

    // The peer saw the NDJSON bundle, authenticated with the supplied token.
    expect(captured).toHaveLength(1);
    expect(captured[0]!.url).toBe("http://peer-b:8787/api/projects/import");
    expect(captured[0]!.auth).toBe("Bearer t-secret");
    const lines = captured[0]!.body.split("\n").filter((l) => l !== "");
    expect(JSON.parse(lines[0]!)).toMatchObject({ type: "project", name: "Shipped" });
    expect(JSON.parse(lines[1]!)).toMatchObject({ type: "session", id: "sess-a" });
  });

  it("404s an unknown project and validates target", async () => {
    const app = createApp(planeOver([]), { meta: freshMeta() });
    expect(
      (await app(post("/api/projects/nope/push", { target: { url: "http://x" } }))).status,
    ).toBe(404);
    const meta = freshMeta();
    const app2 = createApp(planeOver([]), { meta });
    const { project } = (await (await app2(post("/api/projects", { name: "P" }))).json()) as {
      project: { id: string };
    };
    expect((await app2(post(`/api/projects/${project.id}/push`, {}))).status).toBe(400);
    expect(
      (await app2(post(`/api/projects/${project.id}/push`, { target: { url: "x" } }))).status,
    ).toBe(400);
  });

  it("emits an error frame when the target is unreachable", async () => {
    const meta = freshMeta();
    const app = createApp(planeOver([]), {
      meta,
      fetchImpl: (() => Promise.reject(new Error("ECONNREFUSED"))) as typeof fetch,
    });
    const { project } = (await (await app(post("/api/projects", { name: "P" }))).json()) as {
      project: { id: string };
    };
    const res = await app(
      post(`/api/projects/${project.id}/push`, { target: { url: "http://dead:8787" } }),
    );
    const frames = await readSse(res);
    expect(frames.at(-1)?.event).toBe("error");
    expect(String(frames.at(-1)?.data.error)).toContain("ECONNREFUSED");
  });
});

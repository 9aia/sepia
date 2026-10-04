import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Effect } from "effect";
import { describe, expect, it } from "vite-plus/test";
import type { ControlPlaneService } from "sepia-session-control";
import { createApp } from "../src/app";
import { busyFromEvents, createEventFeed, instrumentMeta } from "../src/events";
import { createMetaStore } from "../src/meta";

const plane: ControlPlaneService = {
  listSessions: () => Effect.succeed([]),
  getHistory: () => Effect.succeed({ messages: [], total: 0, start: 0 }),
  createSession: () => Effect.succeed({ id: "sess-new", agentId: "devin" }),
  attach: () => Effect.succeed({ attached: true, readOnly: false, agentId: "devin" }),
  detach: () => Effect.void,
  prompt: () => Effect.void,
  cancel: () => Effect.void,
  deleteSession: () => Effect.void,
  respondToPermission: () => Effect.void,
  subscribe: () => Effect.succeed(() => {}),
  listAgents: () => [{ id: "devin", label: "Devin" }],
  closeAll: () => Effect.void,
};

const tmpMeta = () => {
  const dir = mkdtempSync(join(tmpdir(), "sepia-events-"));
  return createMetaStore(join(dir, "meta.json"));
};

const streamReader = (res: Response) => {
  const reader = res.body?.getReader();
  if (reader === undefined || reader === null) throw new Error("no stream");
  return reader;
};

const readFrame = async (reader: ReturnType<typeof streamReader>): Promise<string> => {
  const { value } = await reader.read();
  return new TextDecoder().decode(value);
};

describe("createEventFeed", () => {
  it("delivers emitted events to subscribers in order", async () => {
    const feed = createEventFeed();
    const sub = feed.subscribe();
    feed.emit("session", { id: "s1", patch: { busy: true } });
    feed.emit("meta", { id: "s1", patch: { pinned: true } });

    await expect(sub.next()).resolves.toEqual({
      kind: "session",
      payload: { id: "s1", patch: { busy: true } },
    });
    await expect(sub.next()).resolves.toEqual({
      kind: "meta",
      payload: { id: "s1", patch: { pinned: true } },
    });
    sub.close();
  });

  it("resolves a pending next() when an event arrives and on close", async () => {
    const feed = createEventFeed();
    const sub = feed.subscribe();
    const pending = sub.next();
    feed.emit("project", { id: "p1", patch: {} });
    await expect(pending).resolves.toEqual({ kind: "project", payload: { id: "p1", patch: {} } });

    const closed = sub.next();
    sub.close();
    await expect(closed).resolves.toBeUndefined();
    await expect(sub.next()).resolves.toBeUndefined();
  });

  it("drops the oldest events when a subscriber falls behind", async () => {
    const feed = createEventFeed();
    const sub = feed.subscribe();
    for (let i = 0; i < 600; i++) feed.emit("session", { n: i });

    const drained: number[] = [];
    for (;;) {
      const event = await sub.next();
      if (event === undefined) break;
      drained.push(event.payload.n as number);
      if (drained.length === 500) break;
    }
    expect(drained).toHaveLength(500);
    // The first 100 were dropped — the queue is bounded at 500.
    expect(drained[0]).toBe(100);
    sub.close();
  });
});

describe("busyFromEvents", () => {
  it("maps the run lifecycle to busy transitions", () => {
    expect(busyFromEvents([{ type: "RUN_STARTED" }])).toBe(true);
    expect(busyFromEvents([{ type: "RUN_FINISHED" }])).toBe(false);
    expect(busyFromEvents([{ type: "RUN_ERROR" }])).toBe(false);
    expect(busyFromEvents([{ type: "RUN_STARTED" }, { type: "RUN_FINISHED" }])).toBe(false);
    expect(busyFromEvents([{ type: "TEXT_MESSAGE_CONTENT" }])).toBeUndefined();
  });
});

describe("instrumentMeta", () => {
  it("emits meta events for session overlay writes", async () => {
    const feed = createEventFeed();
    const sub = feed.subscribe();
    const store = instrumentMeta(tmpMeta(), feed);

    store.patch("s1", { agent: "devin", cwd: "/w" });
    await expect(sub.next()).resolves.toEqual({
      kind: "meta",
      payload: { id: "s1", agent: "devin", patch: { agent: "devin", cwd: "/w" } },
    });

    store.patch("s1", { pinned: true });
    await expect(sub.next()).resolves.toEqual({
      kind: "meta",
      payload: { id: "s1", agent: "devin", patch: { pinned: true } },
    });
    sub.close();
  });

  it("emits a spans patch on addSpan but not on a duplicate append", async () => {
    const feed = createEventFeed();
    const sub = feed.subscribe();
    const store = instrumentMeta(tmpMeta(), feed);

    store.addSpan("s1", { at: 1, agent: "devin", node: "node_a" });
    const first = await sub.next();
    expect(first?.kind).toBe("meta");
    const spans = first === undefined ? [] : (first.payload.patch as { spans: unknown[] }).spans;
    expect(spans).toHaveLength(1);

    // Same agent+node continues the same span — no write, no event.
    store.addSpan("s1", { at: 2, agent: "devin", node: "node_a" });
    store.addSpan("s1", { at: 3, agent: "cline", node: "node_a" });
    const second = await sub.next();
    const spans2 = second === undefined ? [] : (second.payload.patch as { spans: unknown[] }).spans;
    expect(spans2).toHaveLength(2);
    sub.close();
  });

  it("emits meta delete and project lifecycle events", async () => {
    const feed = createEventFeed();
    const sub = feed.subscribe();
    const store = instrumentMeta(tmpMeta(), feed);

    store.patch("s1", { title: "hi" });
    await sub.next();
    store.remove("s1");
    await expect(sub.next()).resolves.toEqual({
      kind: "meta",
      payload: { id: "s1", patch: { deleted: true } },
    });

    const project = store.createProject("web");
    await expect(sub.next()).resolves.toEqual({
      kind: "project",
      payload: { id: project.id, patch: { name: "web" } },
    });

    store.renameProject(project.id, "web-2");
    await expect(sub.next()).resolves.toEqual({
      kind: "project",
      payload: { id: project.id, patch: { name: "web-2" } },
    });

    store.deleteProject(project.id);
    await expect(sub.next()).resolves.toEqual({
      kind: "project",
      payload: { id: project.id, patch: { deleted: true } },
    });
    sub.close();
  });
});

describe("GET /api/events", () => {
  it("streams meta writes as `event: meta` frames", async () => {
    const meta = tmpMeta();
    const app = createApp(plane, { meta, keepAliveMs: 0 });

    const res = await app(new Request("http://localhost/api/events"));
    expect(res.status).toBe(200);
    expect(res.headers.get("content-type")).toBe("text/event-stream");

    await app(
      new Request("http://localhost/api/sessions/s1", {
        method: "PATCH",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ pinned: true }),
      }),
    );

    const reader = streamReader(res);
    const frame = await readFrame(reader);
    const [eventLine, dataLine] = frame.trimEnd().split("\n");
    expect(eventLine).toBe("event: meta");
    expect(JSON.parse(dataLine?.replace(/^data: /, "") ?? "")).toEqual({
      id: "s1",
      patch: { pinned: true },
    });
    await reader.cancel();
  });

  it("streams session lifecycle events", async () => {
    const app = createApp(plane, { meta: tmpMeta(), keepAliveMs: 0 });
    const res = await app(new Request("http://localhost/api/events"));

    await app(
      new Request("http://localhost/api/sessions", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ cwd: "/w" }),
      }),
    );

    const reader = streamReader(res);
    const decoder = new TextDecoder();
    let text = "";
    // The create emits `session` (created) then `meta` (agent/cwd record).
    while (!text.includes("event: session")) {
      text += decoder.decode((await reader.read()).value);
    }
    expect(text).toContain('"created":true');
    await reader.cancel();
  });

  it("emits heartbeat events on the keep-alive interval", async () => {
    const app = createApp(plane, { keepAliveMs: 5 });
    const res = await app(new Request("http://localhost/api/events"));

    const reader = streamReader(res);
    const frame = await readFrame(reader);
    expect(frame).toMatch(/^event: heartbeat\ndata: \{"ts":\d+\}\n\n$/);
    await reader.cancel();
  });

  it("requires the bearer token when one is configured", async () => {
    const app = createApp(plane, { token: "secret", keepAliveMs: 0 });
    const denied = await app(new Request("http://localhost/api/events"));
    expect(denied.status).toBe(401);

    const allowed = await app(new Request("http://localhost/api/events?access_token=secret"));
    expect(allowed.status).toBe(200);
    await streamReader(allowed).cancel();
  });
});

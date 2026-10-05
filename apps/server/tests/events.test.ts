import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Effect } from "effect";
import { describe, expect, it } from "vite-plus/test";
import type { AcpCapabilities } from "sepia-acp";
import type { ControlPlaneService, SessionSummary } from "sepia-session-control";
import { ControlError } from "sepia-session-control";
import { createApp } from "../src/app";
import { busyFromEvents, createEventFeed, instrumentMeta } from "../src/events";
import { createMetaStore } from "../src/meta";

const CAPS: AcpCapabilities = {
  loadSession: true,
  sessionList: true,
  promptCapabilities: { image: true, audio: true, embeddedContext: true },
  sessionCapabilities: {
    list: true,
    delete: true,
    fork: false,
    resume: false,
    close: false,
    additionalDirectories: false,
  },
};

const plane: ControlPlaneService = {
  listSessions: () => Effect.succeed([]),
  getHistory: () => Effect.succeed({ messages: [], total: 0, start: 0 }),
  getSession: () =>
    Effect.fail(new ControlError({ code: "not_found", message: "missing", cause: undefined })),
  createSession: () => Effect.succeed({ id: "sess-new", agentId: "devin", capabilities: CAPS }),
  attach: () =>
    Effect.succeed({ attached: true, readOnly: false, agentId: "devin", capabilities: CAPS }),
  detach: () => Effect.void,
  prompt: () => Effect.void,
  cancel: () => Effect.void,
  deleteSession: () => Effect.void,
  respondToPermission: () => Effect.void,
  restore: () => Effect.succeed({ restored: [], skipped: [] }),
  rewind: () => Effect.succeed({ kept: 0, removed: 0 }),
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

const HELD_SUMMARY: SessionSummary = {
  id: "sess-1",
  title: "Held session",
  cwd: "/work",
  agent: "devin",
  updatedAt: "2024-01-01T00:00:00.000Z",
  locked: true,
  lockHolderPid: 4242,
  source: "devin",
  busy: false,
};

const post = (path: string): Request => new Request(`http://localhost${path}`, { method: "POST" });

/**
 * Reads the stream until `needle` shows up in the accumulated text. Frames
 * are small here so one read is usually one frame — looping just absorbs
 * coalesced chunks.
 */
const readUntil = async (
  reader: ReturnType<typeof streamReader>,
  needle: string,
): Promise<string> => {
  const decoder = new TextDecoder();
  let text = "";
  for (let i = 0; i < 60 && !text.includes(needle); i += 1) {
    const { value, done } = await reader.read();
    if (done === true) break;
    text += decoder.decode(value);
  }
  return text;
};

describe("held-session watch", () => {
  it("emits summary diffs as `session` events, including the release edge", async () => {
    let locked = true;
    let updatedAt = HELD_SUMMARY.updatedAt;
    const heldPlane: ControlPlaneService = {
      ...plane,
      attach: () =>
        Effect.succeed({
          attached: false,
          readOnly: true,
          agentId: "devin",
          capabilities: CAPS,
        }),
      listSessions: (options) =>
        Effect.succeed(
          options?.withLocks === true
            ? [{ ...HELD_SUMMARY, locked, lockHolderPid: locked ? 4242 : null, updatedAt }]
            : [HELD_SUMMARY],
        ),
    };
    const app = createApp(heldPlane, { heldWatchMs: 5, keepAliveMs: 0 });

    const res = await app(new Request("http://localhost/api/events"));
    // A read-only attach is what registers the session as held.
    await app(post("/api/sessions/sess-1/attach"));

    const reader = streamReader(res);
    // First tick diffs against the seeded {locked: true} baseline — the
    // holder's pid and the row's freshness arrive immediately.
    expect(await readUntil(reader, '"lockHolderPid":4242')).toContain('"lockHolderPid":4242');

    // The holder flushing transcript rows bumps updatedAt — the UI's cue to
    // refetch history, no client poll needed.
    updatedAt = "2024-01-01T00:00:05.000Z";
    expect(await readUntil(reader, '"updatedAt":"2024-01-01T00:00:05.000Z"')).toContain(
      "event: session",
    );

    // The release edge — the event that lets a held panel auto-attach.
    locked = false;
    const release = await readUntil(reader, '"locked":false');
    expect(release).toContain('"locked":false');
    expect(release).toContain('"lockHolderPid":null');
    await reader.cancel();
  });

  it("emits locked:false on the first tick when the holder already let go", async () => {
    const heldPlane: ControlPlaneService = {
      ...plane,
      attach: () =>
        Effect.succeed({
          attached: false,
          readOnly: true,
          agentId: "devin",
          capabilities: CAPS,
        }),
      // Attach's own lock probe saw the lock, but the store listing already
      // reads free — the watch still reports the release.
      listSessions: () => Effect.succeed([{ ...HELD_SUMMARY, locked: false, lockHolderPid: null }]),
    };
    const app = createApp(heldPlane, { heldWatchMs: 5, keepAliveMs: 0 });

    const res = await app(new Request("http://localhost/api/events"));
    await app(post("/api/sessions/sess-1/attach"));

    const reader = streamReader(res);
    expect(await readUntil(reader, '"locked":false')).toContain('"locked":false');
    await reader.cancel();
  });

  it("never probes while the feed has no subscribers", async () => {
    let lockLists = 0;
    const heldPlane: ControlPlaneService = {
      ...plane,
      attach: () =>
        Effect.succeed({
          attached: false,
          readOnly: true,
          agentId: "devin",
          capabilities: CAPS,
        }),
      listSessions: (options) =>
        Effect.sync(() => {
          if (options?.withLocks === true) lockLists += 1;
          return [HELD_SUMMARY];
        }),
    };
    const app = createApp(heldPlane, { heldWatchMs: 5, keepAliveMs: 0 });

    await app(post("/api/sessions/sess-1/attach"));
    await new Promise((resolve) => setTimeout(resolve, 40));
    expect(lockLists).toBe(0);
  });
});

it("instrumentMeta suppresses events for no-op removals", async () => {
  const feed = createEventFeed();
  const sub = feed.subscribe();
  const store = instrumentMeta(tmpMeta(), feed);

  // remove/rename/delete of unknown ids must not emit
  store.remove("ghost");
  expect(store.renameProject("ghost", "x")).toBe(false);
  store.deleteProject("ghost");

  const drained = await Promise.race([
    sub.next().then(() => true),
    new Promise<boolean>((resolve) => setTimeout(() => resolve(false), 25)),
  ]);
  expect(drained).toBe(false);
  sub.close();
});

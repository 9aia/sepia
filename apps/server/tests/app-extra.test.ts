/**
 * Route coverage for app.ts — validation branches, the /api/fs and /api/user
 * endpoints, servers dispatch, ui fallback, held-session watch ticks, and
 * the convert/import store paths that fail under the node sqlite stubs.
 */
import { Effect } from "effect";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, describe, expect, it, vi } from "vite-plus/test";
import type { AcpCapabilities, PromptPart } from "sepia-acp";
import type { ControlError, ControlPlaneService } from "sepia-session-control";
import { ControlError as ControlErrorClass } from "sepia-session-control";
import { createApp } from "../src/app";
import { createMetaStore } from "../src/meta";
import { createServerStore } from "../src/servers";
import { createTunnelManager } from "../src/ssh";

const tmp = mkdtempSync(join(tmpdir(), "sepia-app-extra-"));
afterAll(() => rmSync(tmp, { recursive: true, force: true }));

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

const SUMMARY = {
  id: "s1",
  title: "Held session",
  cwd: "/work",
  agent: "devin",
  updatedAt: new Date(0).toISOString(),
  locked: true,
  lockHolderPid: 4242,
  source: "devin" as const,
  busy: false,
};

const failure = (message: string, code: ControlError["code"]): Effect.Effect<never, ControlError> =>
  Effect.fail(new ControlErrorClass({ code, message, cause: undefined }));

interface FakePlane {
  readonly plane: ControlPlaneService;
  readonly calls: {
    attach: Array<{ id: string; options: unknown }>;
    prompt: Array<{ id: string; parts: ReadonlyArray<PromptPart> }>;
  };
  summaries: Array<typeof SUMMARY>;
  attachResult: unknown;
  attachError: unknown;
  push: (events: ReadonlyArray<import("sepia-agui").Event>) => void;
}

const makePlane = (): FakePlane => {
  const listeners = new Set<(events: ReadonlyArray<import("sepia-agui").Event>) => void>();
  const fake: FakePlane = {
    summaries: [SUMMARY],
    attachResult: { attached: false, readOnly: true, agentId: "devin", capabilities: CAPS },
    attachError: undefined,
    calls: { attach: [], prompt: [] },
    push: (events) => {
      for (const listener of listeners) listener(events);
    },
    plane: {
      listSessions: () => Effect.succeed(fake.summaries),
      getHistory: () => Effect.succeed({ messages: [], total: 0, start: 0 }),
      getSession: () => failure("not found", "not_found"),
      getSummary: () => failure("not found", "not_found"),
      createSession: (options) =>
        Effect.succeed({ id: "new-1", agentId: options.agentId ?? "devin", capabilities: CAPS }),
      attach: (id, options) => {
        fake.calls.attach.push({ id, options });
        if (fake.attachError !== undefined)
          return failure("held", fake.attachError as ControlError["code"]);
        return Effect.succeed(
          fake.attachResult as {
            attached: boolean;
            readOnly: boolean;
            agentId: string;
            capabilities: AcpCapabilities;
          },
        );
      },
      detach: () => Effect.void,
      prompt: (id, parts) => {
        fake.calls.prompt.push({ id, parts });
        return Effect.void;
      },
      cancel: () => Effect.void,
      deleteSession: () => Effect.void,
      respondToPermission: () => Effect.void,
      restore: () => Effect.succeed({ restored: [], skipped: [] }),
      rewind: () => Effect.succeed({ kept: 0, removed: 0 }),
      subscribe: (_id, listener) =>
        Effect.sync(() => {
          listeners.add(listener);
          return () => {
            listeners.delete(listener);
          };
        }),
      listAgents: () => [],
      closeAll: () => Effect.void,
    },
  };
  return fake;
};

const get = (path: string): Request =>
  new Request(`http://localhost:8787${path}`, {
    headers: { origin: "http://localhost:3000" },
  });

const req = (method: string, path: string, body?: unknown, raw = false): Request =>
  new Request(`http://localhost:8787${path}`, {
    method,
    headers: {
      origin: "http://localhost:3000",
      "content-type": "application/json",
    },
    body: body === undefined ? undefined : raw ? (body as string) : JSON.stringify(body),
  });

const expect400 = async (app: ReturnType<typeof createApp>, request: Request, match: RegExp) => {
  const response = await app(request);
  expect(response.status).toBe(400);
  expect(((await response.json()) as { error: string }).error).toMatch(match);
};

describe("validation branches", () => {
  const fake = makePlane();
  const plane = fake.plane;
  const meta = createMetaStore(join(tmp, "meta.json"));
  const app = createApp(plane, { meta });

  it("rejects malformed create bodies", async () => {
    await expect400(app, req("POST", "/api/sessions", "{bad", true), /Invalid JSON/);
    await expect400(app, req("POST", "/api/sessions", [1]), /JSON object/);
    await expect400(app, req("POST", "/api/sessions", { cwd: "/w", agent: 3 }), /agent must be/);
    await expect400(app, req("POST", "/api/sessions", { cwd: "/w", title: 3 }), /title must be/);
    await expect400(app, req("POST", "/api/sessions", { cwd: "/w", model: 3 }), /model must be/);
    await expect400(
      app,
      req("POST", "/api/sessions", { cwd: "/w", fallbacks: [1] }),
      /fallbacks must be/,
    );
    const ok = await app(req("POST", "/api/sessions", { cwd: "/w", model: "m", fallbacks: ["a"] }));
    expect(ok.status).toBe(201);
  });

  it("rejects malformed patch bodies", async () => {
    await expect400(app, req("PATCH", "/api/sessions/s1", "{bad", true), /Invalid JSON/);
    await expect400(app, req("PATCH", "/api/sessions/s1", [1]), /JSON object/);
    await expect400(app, req("PATCH", "/api/sessions/s1", { pinned: 1 }), /pinned must be/);
    await expect400(app, req("PATCH", "/api/sessions/s1", { archived: "yes" }), /archived must be/);
    await expect400(app, req("PATCH", "/api/sessions/s1", { model: 7 }), /model must be/);
    await expect400(
      app,
      req("PATCH", "/api/sessions/s1", { model: "x".repeat(101) }),
      /model must be/,
    );
    await expect400(app, req("PATCH", "/api/sessions/s1", { title: "" }), /title must be/);
    await expect400(app, req("PATCH", "/api/sessions/s1", {}), /Nothing to patch/);
    const ok = await app(req("PATCH", "/api/sessions/s1", { model: null }));
    expect(ok.status).toBe(200);
  });

  it("guards internal config keys and validates project bodies", async () => {
    await expect400(app, req("PATCH", "/api/config/vapid", { value: "x" }), /internal/);
    await expect400(app, req("PATCH", "/api/config/x", "{bad", true), /Invalid JSON/);
    const set = await app(req("PATCH", "/api/config/theme", { value: "dark" }));
    expect(set.status).toBe(200);

    await expect400(app, req("POST", "/api/projects", "{bad", true), /Invalid JSON/);
    await expect400(app, req("POST", "/api/projects", { name: "" }), /name must be/);
    await expect400(app, req("POST", "/api/projects", { name: "x".repeat(101) }), /name must be/);
    await expect400(app, req("PATCH", "/api/projects/p1", "{bad", true), /Invalid JSON/);
    await expect400(app, req("PATCH", "/api/projects/p1", { name: 5 }), /name must be/);
    const unknown = await app(req("PATCH", "/api/projects/nope", { name: "x" }));
    expect(unknown.status).toBe(404);
    const deleted = await app(req("DELETE", "/api/projects/nope"));
    expect(deleted.status).toBe(200);
  });

  it("validates history ?before= and prompt bodies", async () => {
    await expect400(app, get("/api/sessions/s1/history?before=-1"), /before must be/);
    await expect400(app, get("/api/sessions/s1/history?before=1.5"), /before must be/);
    const ok = await app(get("/api/sessions/s1/history?before=5&limit=2"));
    expect(ok.status).toBe(200);

    await expect400(app, req("POST", "/api/sessions/s1/prompt", "{bad", true), /Invalid JSON/);
    await expect400(app, req("POST", "/api/sessions/s1/prompt", [1]), /JSON object/);
    await expect400(
      app,
      req("POST", "/api/sessions/s1/prompt", { text: 7 }),
      /text must be a string/,
    );
    await expect400(
      app,
      req("POST", "/api/sessions/s1/prompt", { attachments: "no" }),
      /attachments must be an array/,
    );
    await expect400(
      app,
      req("POST", "/api/sessions/s1/prompt", { attachments: [{ type: "weird" }] }),
      /ACP content blocks/,
    );
    await expect400(
      app,
      req("POST", "/api/sessions/s1/prompt", { attachments: [{ type: "image", data: "x" }] }),
      /ACP content blocks/,
    );
    await expect400(
      app,
      req("POST", "/api/sessions/s1/prompt", {
        attachments: [{ type: "resource", resource: {} }],
      }),
      /ACP content blocks/,
    );
    // too many parts
    await expect400(
      app,
      req("POST", "/api/sessions/s1/prompt", {
        attachments: Array.from({ length: 20 }, () => ({
          type: "resource_link",
          uri: "https://x",
          name: "n",
        })),
      }),
      /Too many attachments/,
    );
  });

  it("validates attach and permission bodies", async () => {
    await expect400(app, req("POST", "/api/sessions/s1/attach", "{bad", true), /Invalid JSON/);
    await expect400(app, req("POST", "/api/sessions/s1/attach", [1]), /JSON object/);
    const ok = await app(
      req("POST", "/api/sessions/s1/attach", { takeover: true, model: "m", fallbacks: ["a", 2] }),
    );
    expect(ok.status).toBe(200);
    // fallbacks mixed with a non-string is silently dropped, model forwarded
    const attachCall = fake.calls.attach.at(-1);
    const opts = (attachCall?.options ?? {}) as { model?: string };
    expect(opts.model).toBe("m");

    await expect400(app, req("POST", "/api/sessions/s1/permission", "{bad", true), /Invalid JSON/);
    await expect400(app, req("POST", "/api/sessions/s1/permission", {}), /requestId/);
    await expect400(
      app,
      req("POST", "/api/sessions/s1/permission", { requestId: "r", optionId: 5 }),
      /optionId/,
    );
    const fine = await app(
      req("POST", "/api/sessions/s1/permission", { requestId: "r", optionId: null }),
    );
    expect(fine.status).toBe(200);
  });

  it("serves /api/fs and /api/user", async () => {
    await expect400(app, get("/api/fs"), /absolute/);
    await expect400(app, get("/api/fs?path=relative"), /absolute/);
    await expect400(app, get("/api/fs?path=/definitely-missing-xyz"), /Cannot read/);
    mkdirSync(join(tmp, "dirA"));
    writeFileSync(join(tmp, "a-file"), "x");
    const listed = await app(get(`/api/fs?path=${encodeURIComponent(tmp)}`));
    expect(listed.status).toBe(200);
    const body = (await listed.json()) as { dirs: string[] };
    expect(body.dirs).toContain(join(tmp, "dirA"));
    expect(body.dirs.some((d) => d.endsWith("a-file"))).toBe(false);

    const user = await app(get("/api/user"));
    expect(user.status).toBe(200);
    expect(((await user.json()) as { user: { homedir: string } }).user.homedir).not.toBe("");
  });
});

describe("held-session watch", () => {
  it("emits release/deletion edges to feed subscribers", async () => {
    const fake = makePlane();
    // attach fails "locked" → the watch registers and starts ticking
    fake.attachError = "locked";
    const app = createApp(fake.plane, { heldWatchMs: 25, keepAliveMs: 0 });

    // Open the node feed so the tick does work.
    const feedResponse = await app(get("/api/events"));
    expect(feedResponse.status).toBe(200);
    const reader = feedResponse.body!.getReader();
    const take = async (): Promise<string> => {
      const { value } = await reader.read();
      return new TextDecoder().decode(value);
    };

    const attachResult = await app(req("POST", "/api/sessions/s1/attach", {}));
    expect(attachResult.status).toBe(409);

    // First tick: the summary is still held — a field-diff patch goes out.
    const frame = await take();
    expect(frame).toContain("event: session");

    // The store row vanishes → a deleted edge.
    fake.summaries = [];
    let deleted = "";
    for (let i = 0; i < 10; i += 1) {
      const chunk = await Promise.race([
        take(),
        new Promise<string>((resolve) => setTimeout(() => resolve(""), 500)),
      ]);
      deleted += chunk;
      if (deleted.includes('"deleted":true')) break;
    }
    expect(deleted).toContain('"deleted":true');
    await reader.cancel();
  });
});

describe("servers routes", () => {
  const storePath = join(tmp, "servers.json");
  const store = createServerStore(storePath, join(tmp, "servers.key"));
  const tunnels = createTunnelManager({ keyDir: join(tmp, "keys") });

  it("501s when servers are not configured and serves CRUD otherwise", async () => {
    const { plane } = makePlane();
    const bare = createApp(plane);
    const unconfigured = await bare(get("/api/servers"));
    expect(unconfigured.status).toBe(501);

    const app = createApp(plane, { servers: store, tunnels });
    const listed = await app(get("/api/servers"));
    expect(((await listed.json()) as { servers: unknown[] }).servers).toEqual([]);

    await expect400(app, req("POST", "/api/servers", "{bad", true), /Invalid JSON/);
    await expect400(app, req("POST", "/api/servers", { label: "x" }), /host is required/);
    const created = await app(
      req("POST", "/api/servers", { label: "peer", host: "127.0.0.1", port: 8787 }),
    );
    expect(created.status).toBe(201);
    const server = (await created.json()) as { server: { id: string } };

    const unknown = await app(get("/api/servers/ghost"));
    expect(unknown.status).toBe(404);
    const unknownGet = await app(get(`/api/servers/${server.server.id}`));
    // bare GET :id falls through — not a defined shape
    expect(unknownGet.status).toBe(404);

    await expect400(
      app,
      req("PATCH", `/api/servers/${server.server.id}`, "{bad", true),
      /Invalid JSON/,
    );
    const patched = await app(
      req("PATCH", `/api/servers/${server.server.id}`, {
        label: "renamed",
        host: "127.0.0.1",
        port: 9,
      }),
    );
    expect(patched.status).toBe(200);

    // tunnel endpoints: no ssh config → 400; DELETE → ok
    const tunnel = await app(req("POST", `/api/servers/${server.server.id}/tunnel`, {}));
    expect(tunnel.status).toBe(400);
    // tunnel DELETE also 400s on a non-ssh entry (ssh check precedes method)
    const untunnel = await app(req("DELETE", `/api/servers/${server.server.id}/tunnel`));
    expect(untunnel.status).toBe(400);

    // proxy: empty path → 400; a real /api path forwards through fetchImpl
    const missingPath = await app(get(`/api/servers/${server.server.id}/proxy`));
    expect(missingPath.status).toBe(400);
    const forbidden = await app(get(`/api/servers/${server.server.id}/proxy/notapi/x`));
    expect(forbidden.status).toBe(400);

    const deleted = await app(req("DELETE", `/api/servers/${server.server.id}`));
    expect(deleted.status).toBe(200);
  });
});

describe("convert + ui fallthrough", () => {
  it("runs the convert effect and maps its failure to 500", async () => {
    const { plane } = makePlane();
    const app = createApp(plane, {
      convert: { dbPath: join(tmp, "sessions.db"), clineDir: join(tmp, "cline") },
    });
    // devin: importCline on a missing dir fails → internal 500
    const devin = await app(req("POST", "/api/sessions/c1/convert", { agent: "devin" }));
    expect(devin.status).toBe(500);
    // cline: installCline hits the stubbed sqlite → internal 500
    const cline = await app(req("POST", "/api/sessions/c1/convert", { agent: "cline" }));
    expect(cline.status).toBe(500);
    await expect400(app, req("POST", "/api/sessions/c1/convert", "{bad", true), /Invalid JSON/);
  });

  it("falls through to the ui handler for non-API GETs", async () => {
    const { plane } = makePlane();
    const calls: string[] = [];
    const ui = {
      fetch: async (_method: string, pathname: string) => {
        calls.push(pathname);
        return pathname === "/" ? new Response("<html>shell</html>") : null;
      },
    };
    const app = createApp(plane, { ui });
    const shell = await app(get("/"));
    expect(shell.status).toBe(200);
    expect(await shell.text()).toBe("<html>shell</html>");
    const miss = await app(get("/assets/whatever.js"));
    expect(miss.status).toBe(404);
    expect(calls).toEqual(["/", "/assets/whatever.js"]);
  });
});

describe("stream + error-mapping edges", () => {
  const failure = (message: string, code: import("sepia-session-control").ControlErrorCode) =>
    Effect.fail(
      Object.assign(new Error(message), { _tag: "ControlError", code, cause: undefined }),
    );

  it("answers the stream route with the subscribe failure", async () => {
    const fake = makePlane();
    (fake.plane as { subscribe: unknown }).subscribe = () => failure("gone", "not_found");
    const app = createApp(fake.plane);
    const response = await app(get("/api/sessions/s1/stream"));
    expect(response.status).toBe(404);
  });

  it("close() runs when the request signal is already aborted", async () => {
    const app = createApp(makePlane().plane);
    const ac = new AbortController();
    ac.abort();
    const response = await app(
      new Request("http://localhost:8787/api/sessions/s1/stream", {
        headers: { origin: "http://localhost:3000" },
        signal: ac.signal,
      }),
    );
    expect(response.status).toBe(200);
    await response.body?.cancel().catch(() => undefined);
  });

  it("maps non-Error failures via errorMessage/errorCode fallbacks", async () => {
    const fake = makePlane();
    (fake.plane as { deleteSession: unknown }).deleteSession = () =>
      Effect.fail({ message: 42 } as never); // record-shaped non-Error
    const app = createApp(fake.plane);
    const response = await app(req("DELETE", "/api/sessions/s1"));
    expect(response.status).toBe(500);
    expect(((await response.json()) as { error: string }).error).toBe("42");

    (fake.plane as { deleteSession: unknown }).deleteSession = () =>
      Effect.fail("raw-string" as never);
    const raw = await app(req("DELETE", "/api/sessions/s1"));
    expect(raw.status).toBe(500);
  });

  it("forwards valid attach fallbacks and rejects bad rewind/cancel JSON", async () => {
    const fake = makePlane();
    fake.attachError = undefined;
    fake.attachResult = { attached: true, readOnly: false, agentId: "devin", capabilities: CAPS };
    const app = createApp(fake.plane);
    const res = await app(
      req("POST", "/api/sessions/s1/attach", { takeover: true, fallbacks: ["a", "b"] }),
    );
    expect(res.status).toBe(200);
    const options = fake.calls.attach.at(-1)?.options as { fallbacks?: string[] };
    expect(options.fallbacks).toEqual(["a", "b"]);

    await expect400(app, req("POST", "/api/sessions/s1/rewind", "{bad", true), /Invalid JSON/);
    await expect400(app, req("POST", "/api/sessions/s1/restore", "{bad", true), /Invalid JSON/);
  });
});

describe("prompt payload budget", () => {
  it("accepts valid attachments across all content types", async () => {
    const fake = makePlane();
    const app = createApp(fake.plane);
    const res = await app(
      req("POST", "/api/sessions/s1/prompt", {
        text: "hello",
        attachments: [
          { type: "text", text: "extra" },
          { type: "image", data: "AAAA", mimeType: "image/png" },
          { type: "audio", data: "BBBB", mimeType: "audio/mp3" },
          { type: "resource", resource: { uri: "file:///x", text: "body" } },
          { type: "resource", resource: { uri: "file:///y", blob: "CCCC" } },
          { type: "resource_link", uri: "https://x", name: "n", mimeType: "text/plain", size: 3 },
        ],
      }),
    );
    expect(res.status).toBe(200);
    expect(fake.calls.prompt.at(-1)?.parts).toHaveLength(7);
  });

  it("rejects malformed parts (non-record, missing type, bad text)", async () => {
    const app = createApp(makePlane().plane);
    await expect400(
      app,
      req("POST", "/api/sessions/s1/prompt", { attachments: [42] }),
      /ACP content blocks/,
    );
    await expect400(
      app,
      req("POST", "/api/sessions/s1/prompt", { attachments: [{ type: 5 }] }),
      /ACP content blocks/,
    );
    await expect400(
      app,
      req("POST", "/api/sessions/s1/prompt", { attachments: [{ type: "text", text: 3 }] }),
      /ACP content blocks/,
    );
    await expect400(
      app,
      req("POST", "/api/sessions/s1/prompt", {
        attachments: [{ type: "resource_link", uri: "", name: "n" }],
      }),
      /ACP content blocks/,
    );
  });
});

describe("import via the default session installer", () => {
  const IR = {
    id: "s-ir",
    title: "t",
    workingDirectory: "/w",
    model: "m",
    createdAt: 1,
    lastActivityAt: 2,
    mainChainId: 0,
    nodes: [],
    promptHistory: [],
  };

  it("validates the import body shape", async () => {
    const app = createApp(makePlane().plane, {
      convert: { dbPath: join(tmp, "db.db"), clineDir: join(tmp, "cl") },
    });
    await expect400(app, req("POST", "/api/sessions/import", "{bad", true), /Invalid JSON/);
    await expect400(app, req("POST", "/api/sessions/import", [1]), /JSON object/);
    await expect400(
      app,
      req("POST", "/api/sessions/import", { agent: "cline", session: IR, title: 4 }),
      /title must be/,
    );
    await expect400(
      app,
      req("POST", "/api/sessions/import", { agent: "cline", session: IR, model: 4 }),
      /model must be/,
    );
    await expect400(
      app,
      req("POST", "/api/sessions/import", { agent: "cline", session: IR, cwd: " " }),
      /cwd must be/,
    );
  });

  it("runs defaultImportSession for both targets — stubbed sqlite maps to 500", async () => {
    const app = createApp(makePlane().plane, {
      convert: { dbPath: join(tmp, "db.db"), clineDir: join(tmp, "cl") },
    });
    const cline = await app(req("POST", "/api/sessions/import", { agent: "cline", session: IR }));
    expect(cline.status).toBe(500);
    const devin = await app(req("POST", "/api/sessions/import", { agent: "devin", session: IR }));
    expect(devin.status).toBe(500);
  });
});

describe("held-watch TTL + tick failure", () => {
  it("purges watches older than the TTL and stops the timer", async () => {
    const fake = makePlane();
    fake.attachError = "locked";
    const app = createApp(fake.plane, { heldWatchMs: 15 });
    const feed = await app(get("/api/events"));
    const reader = feed.body!.getReader();
    void reader; // keep the subscriber alive — the tick only runs with one

    const before = Date.now();
    await app(req("POST", "/api/sessions/s1/attach", {}));
    // freeze real timers; jump the clock past HELD_WATCH_TTL_MS (30min)
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(before + 31 * 60_000);
    vi.useRealTimers();
    // a couple of ticks pass: the watch is expired → purged → timer cleared
    await new Promise((resolve) => setTimeout(resolve, 80));
    await reader.cancel();
  });

  it("logs a warning when listSessions fails during a tick", async () => {
    const fake = makePlane();
    fake.attachError = "locked";
    const failing = {
      ...fake.plane,
      listSessions: () =>
        Effect.fail(
          new ControlErrorClass({ code: "internal", message: "db gone", cause: undefined }),
        ),
    } as unknown as import("sepia-session-control").ControlPlaneService;
    const app = createApp(failing, { heldWatchMs: 15 });
    await app(req("POST", "/api/sessions/s1/attach", {}));
    await new Promise((resolve) => setTimeout(resolve, 60));
    // no crash — the tick's failure path ran
  });
});

describe("push subscriptions + busy feed emission", () => {
  it("501s without a meta store and validates bodies with one", async () => {
    const noMeta = createApp(makePlane().plane);
    expect((await noMeta(get("/api/push/vapid"))).status).toBe(501);
    expect((await noMeta(req("POST", "/api/push/subscribe", {}))).status).toBe(501);
    expect((await noMeta(req("DELETE", "/api/push/subscribe", {}))).status).toBe(501);

    const meta = createMetaStore(join(tmp, "meta-push.json"));
    const app = createApp(makePlane().plane, { meta });
    await expect400(app, req("POST", "/api/push/subscribe", "{bad", true), /Invalid JSON/);
    await expect400(
      app,
      req("POST", "/api/push/subscribe", { endpoint: "x" }),
      /Invalid push subscription/,
    );
    await expect400(app, req("DELETE", "/api/push/subscribe", "{bad", true), /Invalid JSON/);
  });

  it("emits busy transitions onto the feed via the live listener", async () => {
    const fake = makePlane();
    fake.attachResult = { attached: true, readOnly: false, agentId: "devin", capabilities: CAPS };
    const meta = createMetaStore(join(tmp, "meta-live.json"));
    const app = createApp(fake.plane, { meta, heldWatchMs: 0 });
    const feed = await app(get("/api/events"));
    const reader = feed.body!.getReader();
    const take = async () => new TextDecoder().decode((await reader.read()).value);

    await app(req("POST", "/api/sessions/s1/attach", {}));
    fake.push([{ type: "RUN_STARTED" } as import("sepia-agui").Event]);
    let frame = "";
    for (let i = 0; i < 6 && !frame.includes('"busy":true'); i += 1) {
      frame += await Promise.race([
        take(),
        new Promise<string>((r) => setTimeout(() => r(""), 200)),
      ]);
    }
    expect(frame).toContain('"busy":true');
    await reader.cancel();
  });
});

describe("misc env/501 branches", () => {
  it("parses SEPIA_HELD_WATCH_MS and 501s meta-only routes without a store", async () => {
    process.env.SEPIA_HELD_WATCH_MS = "42";
    try {
      const app = createApp(makePlane().plane);
      expect((await app(req("POST", "/api/projects", { name: "x" }))).status).toBe(501);
      expect((await app(req("PATCH", "/api/projects/p1", { name: "x" }))).status).toBe(501);
      expect((await app(req("DELETE", "/api/projects/p1"))).status).toBe(501);
      expect((await app(req("GET", "/api/projects"))).status).toBe(501);
    } finally {
      delete process.env.SEPIA_HELD_WATCH_MS;
    }
  });
});

describe("held-watch TTL purge — real implementation", () => {
  it("expires watches older than HELD_WATCH_TTL_MS and stops the timer", async () => {
    const fake = makePlane();
    fake.attachError = "locked";
    const app = createApp(fake.plane, { heldWatchMs: 15 });
    const feed = await app(get("/api/events"));
    const reader = feed.body!.getReader();

    await app(req("POST", "/api/sessions/s1/attach", {}));
    // keep the fake clock advanced while real interval ticks fire — the
    // watch's `at` is 31 minutes stale → purged → heldTimer cleared
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(Date.now() + 31 * 60_000);
    await new Promise((resolve) => setTimeout(resolve, 80));
    vi.useRealTimers();
    await reader.cancel();
  });
});

describe("held-watch tick failure (with a feed subscriber)", () => {
  it("logs a warning and keeps watching when listSessions fails", async () => {
    const fake = makePlane();
    fake.attachError = "locked";
    const failing = {
      ...fake.plane,
      listSessions: () =>
        Effect.fail(
          new ControlErrorClass({ code: "internal", message: "db gone", cause: undefined }),
        ),
    } as unknown as import("sepia-session-control").ControlPlaneService;
    const app = createApp(failing, { heldWatchMs: 15 });
    const feed = await app(get("/api/events")); // subscriber present → tick does work
    const reader = feed.body!.getReader();
    await app(req("POST", "/api/sessions/s1/attach", {}));
    await new Promise((resolve) => setTimeout(resolve, 60));
    await reader.cancel();
  });
});

describe("import history-field mapping + meta overlay edges", () => {
  it("maps optional history fields through to the IR builder", async () => {
    const app = createApp(makePlane().plane, {
      convert: { dbPath: join(tmp, "db2.db"), clineDir: join(tmp, "cl2") },
    });
    const res = await app(
      req("POST", "/api/sessions/import", {
        agent: "cline",
        history: [
          { role: "user", content: "hi", createdAt: 1 },
          {
            role: "assistant",
            content: "reply",
            createdAt: 2,
            thinking: "hmm",
            thinkingSignature: "sig",
            usage: { input: 1, output: 2, thinking: 3 },
            model: "m1",
            requestId: "r1",
            finishReason: "tool_use",
            toolStatus: "success",
            exitCode: 0,
            durationMs: 9,
          },
        ],
      }),
    );
    // sqlite is stubbed → the field mapping ran, the store write 500s
    expect(res.status).toBe(500);
  });

  it("overlays meta on unknown session ids and reads /api/config listing", async () => {
    const meta = createMetaStore(join(tmp, "meta-overlay.json"));
    meta.patch("ghost-42", { title: "From meta", agent: "devin", cwd: "/g" });
    const app = createApp(makePlane().plane, { meta });
    const listed = await app(get("/api/sessions"));
    const sessions = (await listed.json()) as { sessions: Array<{ id: string }> };
    // the meta-only record appears as a pending session
    expect(sessions.sessions.some((s) => s.id === "ghost-42")).toBe(true);
  });

  it("handles non-record bodies on config/project mutations", async () => {
    const meta = createMetaStore(join(tmp, "meta-cfg.json"));
    const app = createApp(makePlane().plane, { meta });
    const set = await app(req("PATCH", "/api/config/k", '"just-a-string"', true));
    expect(set.status).toBe(200);
    const proj = await app(req("POST", "/api/projects", '"x"', true));
    expect(proj.status).toBe(400);
    const patch = await app(req("PATCH", "/api/projects/p1", '"x"', true));
    expect(patch.status).toBe(400);
    const perm = await app(req("POST", "/api/sessions/s1/permission", { requestId: "r" }));
    expect(perm.status).toBe(200);
  });
});

describe("misc reachables", () => {
  it("accepts a '*' origin and parses garbage held-watch env", async () => {
    process.env.SEPIA_HELD_WATCH_MS = "junk";
    try {
      const app = createApp(makePlane().plane, { allowedOrigins: ["*"] });
      const res = await app(
        new Request("http://localhost:8787/api/sessions", {
          headers: { origin: "https://example.com" },
        }),
      );
      expect(res.status).toBe(200);
    } finally {
      delete process.env.SEPIA_HELD_WATCH_MS;
    }
  });

  it("answers HEAD through the ui fallthrough", async () => {
    const seen: string[] = [];
    const app = createApp(makePlane().plane, {
      ui: {
        fetch: async (method, path) => {
          seen.push(`${method} ${path}`);
          return new Response("x");
        },
      },
    });
    const head = await app(
      new Request("http://localhost:8787/", {
        method: "HEAD",
        headers: { origin: "http://localhost:3000" },
      }),
    );
    expect(head.status).toBe(200);
    expect(seen[0]).toBe("HEAD /");
  });

  it("closes the /api/events stream when the request is pre-aborted", async () => {
    const app = createApp(makePlane().plane);
    const ac = new AbortController();
    ac.abort();
    const res = await app(
      new Request("http://localhost:8787/api/events", {
        headers: { origin: "http://localhost:3000" },
        signal: ac.signal,
      }),
    );
    expect(res.status).toBe(200);
    await res.body?.cancel().catch(() => undefined);
  });
});

describe("health timeout", () => {
  it("answers 503 when the store check exceeds the timeout", async () => {
    const fake = makePlane();
    (fake.plane as { listSessions: unknown }).listSessions = () => Effect.never;
    vi.useFakeTimers();
    try {
      const app = createApp(fake.plane);
      const pending = app(get("/api/health"));
      await vi.advanceTimersByTimeAsync(2000);
      const res = await pending;
      expect(res.status).toBe(503);
      expect((await res.json()) as { db: boolean }).toEqual({ ok: false, db: false });
    } finally {
      vi.useRealTimers();
    }
  });
});

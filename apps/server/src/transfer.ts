import { Either, Effect } from "effect";
import { sseHeaders } from "sepia-agui";
import { Conversion } from "sepia-convert";
import { Session } from "sepia-core";
import type { ControlPlaneService } from "sepia-session-control";
import { sessionPayload, type NodeEventFeed } from "./events";
import { isRunSpan, type MetaStore, type SessionMeta } from "./meta";
import type { NodeIdentity } from "./node";
import { SseChannel } from "./sse-channel";

/**
 * Distributed project transfer (docs/protocol.md "Project transfer"). A
 * project — its meta row plus every member session's full IR — serializes
 * as `application/x-ndjson`: one JSON object per line so arbitrarily large
 * projects stream without buffering.
 *
 * Bundle lines:
 *
 *   {"type":"project","version":1,"id":"…","name":"…","node":{…},"sessions":N}
 *   {"type":"session","id":"<source id>","agent":"cline","title":"…",
 *     "meta":{…SessionMeta…},"session":{…SessionJson…}}
 *   {"type":"skipped","id":"…","error":"…"}   — a member the source couldn't read
 *   {"type":"end","sessions":N,"skipped":M}
 *
 * The header is always first; unknown line types are ignored so newer nodes
 * can extend the format without breaking older importers.
 */
export const BUNDLE_VERSION = 1;
export const BUNDLE_CONTENT_TYPE = "application/x-ndjson";

/** `(session, targetAgent) → stored id` — the same seam as POST /api/sessions/import. */
export type ImportExecutor = (
  session: Session,
  agent: "cline" | "devin",
) => Effect.Effect<string, unknown>;

/** Local copy of app.ts's EffectRunner — transfer.ts must not import the router module. */
type Run = <A, E>(effect: Effect.Effect<A, E>) => Promise<A>;

const encoder = new TextEncoder();

const line = (value: unknown): Uint8Array => encoder.encode(`${JSON.stringify(value)}\n`);

const errorMessage = (error: unknown): string =>
  error instanceof Error ? error.message : String(error);

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

/**
 * The target store for a bundle session — only the writable agent stores
 * take imports. Cline sessions stay Cline; every other agent's IR lands in
 * the Devin store, which round-trips the full tree losslessly.
 */
export const targetAgentFor = (sourceAgent: string): "cline" | "devin" =>
  sourceAgent === "cline" ? "cline" : "devin";

/** The meta overlay fields that travel with a session (projectIds don't — they're node-local). */
const transferableMeta = (meta: SessionMeta | undefined): Record<string, unknown> => {
  if (meta === undefined) return {};
  const out: Record<string, unknown> = {};
  if (meta.title !== undefined) out.title = meta.title;
  if (meta.pinned !== undefined) out.pinned = meta.pinned;
  if (meta.archived !== undefined) out.archived = meta.archived;
  if (meta.model !== undefined) out.model = meta.model;
  if (meta.spans !== undefined) out.spans = meta.spans;
  if (meta.agent !== undefined) out.agent = meta.agent;
  if (meta.cwd !== undefined) out.cwd = meta.cwd;
  if (meta.createdAt !== undefined) out.createdAt = meta.createdAt;
  return out;
};

/**
 * `GET /api/projects/:id/export`'s body — a lazily produced NDJSON stream:
 * the member list is resolved up front (so the header can carry the count),
 * then each session's IR is fetched and encoded one line at a time. A member
 * whose store read fails becomes a `skipped` line rather than aborting the
 * bundle. `onLine` taps each emitted record — the push route turns it into
 * progress events.
 */
export const projectBundleStream = (options: {
  readonly plane: ControlPlaneService;
  readonly run: Run;
  readonly meta: MetaStore;
  readonly project: { readonly id: string; readonly name: string };
  readonly node: NodeIdentity;
  readonly onLine?: (record: Record<string, unknown>) => void;
}): ReadableStream<Uint8Array> =>
  new ReadableStream<Uint8Array>({
    async start(controller) {
      const emit = (record: Record<string, unknown>): void => {
        options.onLine?.(record);
        controller.enqueue(line(record));
      };
      try {
        const listed = await options.run(Effect.either(options.plane.listSessions()));
        if (Either.isLeft(listed)) throw listed.left;
        const members = listed.right.filter(
          (session) =>
            options.meta.of(session.id)?.projectIds?.includes(options.project.id) === true,
        );
        emit({
          type: "project",
          version: BUNDLE_VERSION,
          id: options.project.id,
          name: options.project.name,
          node: { id: options.node.id, name: options.node.name },
          sessions: members.length,
        });
        let written = 0;
        let skipped = 0;
        for (const member of members) {
          try {
            const session = await options.run(
              options.plane.getSession(member.id, { agentId: member.agent }),
            );
            const meta = options.meta.of(member.id);
            emit({
              type: "session",
              id: member.id,
              agent: member.agent,
              title: meta?.title ?? session.title,
              meta: transferableMeta(meta),
              session: Conversion.sessionToJson(session),
            });
            written += 1;
          } catch (error) {
            emit({ type: "skipped", id: member.id, error: errorMessage(error) });
            skipped += 1;
          }
        }
        emit({ type: "end", sessions: written, skipped });
        controller.close();
      } catch (error) {
        controller.error(error);
      }
    },
  });

/** NDJSON reader — yields each non-empty line as raw text; never buffers the whole body. */
export const readNdjsonLines = async function* (
  body: ReadableStream<Uint8Array>,
): AsyncGenerator<string> {
  const reader = body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      buffer += decoder.decode(value, { stream: true });
      let at: number;
      while ((at = buffer.indexOf("\n")) !== -1) {
        const part = buffer.slice(0, at);
        buffer = buffer.slice(at + 1);
        if (part.trim() !== "") yield part;
      }
    }
    buffer += decoder.decode();
    if (buffer.trim() !== "") yield buffer;
  } finally {
    reader.releaseLock();
  }
};

export interface ImportedSessionRow {
  /** The id in the local store (a cline target may mint its own). */
  readonly id: string;
  /** The id the session carried on the source node. */
  readonly sourceId: string;
  readonly agent: string;
  readonly title: string;
}

export interface SkippedSessionRow {
  readonly id: string;
  readonly error: string;
}

export interface ProjectImportResult {
  readonly project: { readonly id: string; readonly name: string };
  readonly imported: ReadonlyArray<ImportedSessionRow>;
  readonly skipped: ReadonlyArray<SkippedSessionRow>;
  /** True when the stream ended without the `end` line — the bundle may be partial. */
  readonly truncated: boolean;
}

export interface ImportProgress {
  readonly index: number;
  readonly total: number | undefined;
  readonly id: string;
  readonly sourceId: string;
  readonly agent: string;
  readonly title: string;
}

/**
 * Consume a bundle body: the `project` line creates/refreshes the local
 * project under its source id (idempotent — a re-pull is an update, not a
 * clone), then each `session` line goes through the same executor as
 * `POST /api/sessions/import` and gets the meta overlay (title/pin/archive/
 * model/spans) re-pointed at the project. A malformed session line lands in
 * `skipped`, never aborts the rest. Feed events fire per landed row so
 * connected UIs update live.
 */
export const importProjectBundle = async (
  body: ReadableStream<Uint8Array>,
  deps: {
    readonly executor: ImportExecutor;
    readonly run: Run;
    readonly meta: MetaStore;
    readonly feed: NodeEventFeed;
    readonly node: Pick<NodeIdentity, "id">;
    readonly onSession?: (progress: ImportProgress) => void;
  },
): Promise<ProjectImportResult> => {
  let header: { readonly id: string; readonly name: string } | undefined;
  let total: number | undefined;
  const imported: ImportedSessionRow[] = [];
  const skipped: SkippedSessionRow[] = [];
  let index = 0;
  let sawEnd = false;

  for await (const text of readNdjsonLines(body)) {
    let record: unknown;
    try {
      record = JSON.parse(text);
    } catch {
      throw new Error("Invalid project bundle: a line is not JSON");
    }
    if (!isRecord(record) || typeof record.type !== "string") {
      throw new Error("Invalid project bundle: malformed line");
    }
    if (record.type === "project") {
      if (header !== undefined) throw new Error("Invalid bundle: duplicate project line");
      if (
        typeof record.id !== "string" ||
        record.id === "" ||
        typeof record.name !== "string" ||
        record.name === ""
      ) {
        throw new Error("Invalid bundle: the project line needs id and name");
      }
      header = { id: record.id, name: record.name };
      deps.meta.ensureProject(header.id, header.name);
      total = typeof record.sessions === "number" ? record.sessions : undefined;
      continue;
    }
    if (header === undefined) {
      throw new Error("Invalid project bundle: expected the project line first");
    }
    if (record.type === "session") {
      index += 1;
      const sourceId = typeof record.id === "string" ? record.id : `line-${index}`;
      try {
        const decoded = Conversion.sessionFromJson(record.session);
        const sourceAgent = typeof record.agent === "string" ? record.agent : "devin";
        const agent = targetAgentFor(sourceAgent);
        const meta = isRecord(record.meta) ? record.meta : {};
        const storedId = await deps.run(deps.executor(decoded, agent));
        const title =
          typeof meta.title === "string" && meta.title !== "" ? meta.title : decoded.title;
        const spans = Array.isArray(meta.spans) ? meta.spans.filter(isRunSpan) : [];
        deps.feed.emit("session", sessionPayload(storedId, agent, { created: true }));
        deps.meta.patch(storedId, {
          ...(typeof meta.title === "string" && meta.title !== "" ? { title: meta.title } : {}),
          ...(meta.pinned === true ? { pinned: true } : {}),
          ...(meta.archived === true ? { archived: true } : {}),
          ...(meta.model === null || typeof meta.model === "string" ? { model: meta.model } : {}),
          spans,
          agent,
          cwd: decoded.workingDirectory,
          ...(typeof meta.createdAt === "string" ? { createdAt: meta.createdAt } : {}),
          // Re-point membership at the local project — and keep any local
          // memberships a re-pull already recorded.
          projectIds: [...new Set([...(deps.meta.of(storedId)?.projectIds ?? []), header.id])],
        });
        // Provenance: the imported copy's next run continues under this
        // node's control plane — same record an attach would write.
        deps.meta.addSpan(storedId, { at: Date.now(), agent, node: deps.node.id });
        imported.push({ id: storedId, sourceId, agent, title });
        deps.onSession?.({ index, total, id: storedId, sourceId, agent, title });
      } catch (error) {
        skipped.push({ id: sourceId, error: errorMessage(error) });
      }
      continue;
    }
    if (record.type === "skipped") {
      skipped.push({
        id: typeof record.id === "string" ? record.id : "?",
        error: typeof record.error === "string" ? record.error : "export failed on the source",
      });
      continue;
    }
    if (record.type === "end") {
      sawEnd = true;
      continue;
    }
    // Unknown line types are ignored — forward compatibility.
  }
  if (header === undefined) {
    throw new Error("Not a project bundle — missing the project line");
  }
  return { project: header, imported, skipped, truncated: !sawEnd };
};

/** `{url, token?}` — how a client hands a peer's address + credential to a node for pull/push. */
export interface RemoteEndpoint {
  readonly url: string;
  readonly token?: string;
}

/**
 * Validate a client-supplied peer address: an absolute http(s) URL — the
 * node fetches it itself (node-to-node), so a bare host or a ws:// won't do.
 * Returns the canonical origin (path is dropped: only /api/* endpoints are
 * ever appended).
 */
export const parseRemoteUrl = (value: unknown): string | Error => {
  if (typeof value !== "string" || value.trim() === "") {
    return new Error("url must be a non-empty string");
  }
  let url;
  try {
    url = new URL(value.trim());
  } catch {
    return new Error("url must be an absolute http(s) URL");
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") {
    return new Error("url must be an http(s) URL");
  }
  return url.origin;
};

const remoteAuth = (endpoint: RemoteEndpoint): Record<string, string> =>
  endpoint.token !== undefined && endpoint.token !== ""
    ? { authorization: `Bearer ${endpoint.token}` }
    : {};

/** Pull half: fetch the remote's `GET /api/projects/:id/export` stream. */
export const fetchProjectBundle = async (
  source: RemoteEndpoint,
  projectId: string,
  fetchImpl: typeof fetch,
  signal: AbortSignal,
): Promise<ReadableStream<Uint8Array> | Error> => {
  let res: Response;
  try {
    res = await fetchImpl(`${source.url}/api/projects/${encodeURIComponent(projectId)}/export`, {
      headers: { accept: BUNDLE_CONTENT_TYPE, ...remoteAuth(source) },
      signal,
    });
  } catch (error) {
    return new Error(`Cannot reach the source node: ${errorMessage(error)}`);
  }
  if (!res.ok) {
    let detail = `the source responded ${res.status}`;
    try {
      const body: unknown = await res.json();
      if (isRecord(body) && typeof body.error === "string") detail = body.error;
    } catch {
      // Non-JSON error body — keep the status text.
    }
    return new Error(`Source refused the export: ${detail}`);
  }
  if (res.body === null) return new Error("The source's export had no body");
  return res.body;
};

/** Push half: POST a bundle stream to the remote's `POST /api/projects/import`. */
export const postProjectBundle = async (
  target: RemoteEndpoint,
  body: ReadableStream<Uint8Array>,
  fetchImpl: typeof fetch,
  signal: AbortSignal,
): Promise<Record<string, unknown> | Error> => {
  let res: Response;
  try {
    res = await fetchImpl(`${target.url}/api/projects/import`, {
      method: "POST",
      headers: { "content-type": BUNDLE_CONTENT_TYPE, ...remoteAuth(target) },
      body,
      // A ReadableStream body requires half-duplex upload (undici/Bun).
      duplex: "half",
      signal,
    } as RequestInit);
  } catch (error) {
    return new Error(`Cannot reach the target node: ${errorMessage(error)}`);
  }
  if (!res.ok) {
    let detail = `the target responded ${res.status}`;
    try {
      const parsed: unknown = await res.json();
      if (isRecord(parsed) && typeof parsed.error === "string") detail = parsed.error;
    } catch {
      // Non-JSON error body — keep the status text.
    }
    return new Error(`Target refused the import: ${detail}`);
  }
  try {
    return (await res.json()) as Record<string, unknown>;
  } catch {
    return new Error("The target's import response was not JSON");
  }
};

/**
 * Session transfer — the single-session counterpart of the project bundle
 * verbs. The wire format is the existing session IR JSON (`{session}` from
 * `GET /api/sessions/:id/export`, consumed verbatim by
 * `POST /api/sessions/import`), so each hop just chains those endpoints
 * node-to-node; no new format or route is needed on the peer.
 */

/**
 * Pull half: fetch the remote's `GET /api/sessions/:id/export` `{session}`
 * payload and decode the IR — a peer that answers 200 with a non-IR body is
 * reported like any other refusal.
 */
export const fetchSessionExport = async (
  source: RemoteEndpoint,
  sessionId: string,
  agentId: string | undefined,
  fetchImpl: typeof fetch,
  signal: AbortSignal,
): Promise<Session | Error> => {
  const agent = agentId === undefined ? "" : `?agent=${encodeURIComponent(agentId)}`;
  let res: Response;
  try {
    res = await fetchImpl(
      `${source.url}/api/sessions/${encodeURIComponent(sessionId)}/export${agent}`,
      {
        headers: { accept: "application/json", ...remoteAuth(source) },
        signal,
      },
    );
  } catch (error) {
    return new Error(`Cannot reach the source node: ${errorMessage(error)}`);
  }
  if (!res.ok) {
    let detail = `the source responded ${res.status}`;
    try {
      const body: unknown = await res.json();
      if (isRecord(body) && typeof body.error === "string") detail = body.error;
    } catch {
      // Non-JSON error body — keep the status text.
    }
    return new Error(`Source refused the export: ${detail}`);
  }
  let body: unknown;
  try {
    body = await res.json();
  } catch {
    return new Error("The source's export was not JSON");
  }
  if (!isRecord(body) || body.session === undefined) {
    return new Error("The source's export carried no session payload");
  }
  try {
    return Conversion.sessionFromJson(body.session);
  } catch {
    return new Error("The source's export was not a session IR object");
  }
};

/** Push half: POST `{agent, session, title?, model?}` to the remote's `POST /api/sessions/import`. */
export const postSessionImport = async (
  target: RemoteEndpoint,
  payload: Record<string, unknown>,
  fetchImpl: typeof fetch,
  signal: AbortSignal,
): Promise<Record<string, unknown> | Error> => {
  let res: Response;
  try {
    res = await fetchImpl(`${target.url}/api/sessions/import`, {
      method: "POST",
      headers: { "content-type": "application/json", ...remoteAuth(target) },
      body: JSON.stringify(payload),
      signal,
    });
  } catch (error) {
    return new Error(`Cannot reach the target node: ${errorMessage(error)}`);
  }
  if (!res.ok) {
    let detail = `the target responded ${res.status}`;
    try {
      const parsed: unknown = await res.json();
      if (isRecord(parsed) && typeof parsed.error === "string") detail = parsed.error;
    } catch {
      // Non-JSON error body — keep the status text.
    }
    return new Error(`Target refused the import: ${detail}`);
  }
  try {
    return (await res.json()) as Record<string, unknown>;
  } catch {
    return new Error("The target's import response was not JSON");
  }
};

export type TransferEmit = (event: string, data: unknown) => void;

/**
 * The pull/push response: an SSE stream of `start` / `session` progress
 * frames ending in `done` (the import summary) or `error`. `work` runs once
 * the stream exists — the SseChannel buffers pre-start frames, so nothing
 * races the client opening it.
 */
export const transferSseResponse = (
  work: (emit: TransferEmit) => Promise<void>,
  signal: AbortSignal,
  cors: Record<string, string>,
  keepAliveMs: number,
): Response => {
  const channel = new SseChannel({ keepAliveMs });
  const emit: TransferEmit = (event, data) =>
    channel.push(`event: ${event}\ndata: ${JSON.stringify(data)}\n\n`);

  const stream = new ReadableStream<Uint8Array>(
    {
      start(controller) {
        channel.start(controller);
        const close = (): void => {
          channel.close();
          try {
            controller.close();
          } catch {
            // Already closed by the consumer.
          }
        };
        if (signal.aborted) close();
        else signal.addEventListener("abort", close, { once: true });
        void work(emit)
          .catch((error: unknown) => emit("error", { error: errorMessage(error) }))
          .finally(() => channel.close());
      },
      pull() {
        channel.onPull();
      },
      cancel() {
        channel.close();
      },
    },
    { highWaterMark: 1 },
  );

  return new Response(stream, { headers: { ...sseHeaders, ...cors } });
};

import { randomUUID } from "node:crypto";
import { join } from "node:path";
import * as BunFileSystem from "@effect/platform-bun/BunFileSystem";
import * as BunPath from "@effect/platform-bun/BunPath";
import { Effect, Either, Layer, Schema } from "effect";
import { encodeSse, sseHeaders, type Event } from "sepia-agui";
import { Block, Session } from "sepia-core";
import { Conversion, ClineStore } from "sepia-convert";
import { openSessionsDb, SqliteStorage } from "sepia-devin";
import { ControlError } from "sepia-session-control";
import type { PromptPart } from "sepia-acp";
import type { ControlPlaneService, SessionEventListener, Unsubscribe } from "sepia-session-control";
import { sessionPayload, type NodeEventFeed } from "../events";
import type { MetaStore } from "../meta";
import type { NodeIdentity } from "../node";
import { SseChannel } from "../sse-channel";
import {
  defaultImportSession,
  errorMessage,
  errorResponse,
  isRecord,
  jsonResponse,
  readJsonBody,
  respond,
  segmentsEqual,
  type EffectRunner,
  type RouteHandler,
} from "./shared";

const streamResponse = async (
  run: EffectRunner,
  plane: ControlPlaneService,
  id: string,
  signal: AbortSignal,
  cors: Record<string, string>,
  keepAliveMs: number,
  agentId?: string,
): Promise<Response> => {
  let unsubscribe: Unsubscribe = () => {};
  const channel = new SseChannel({
    keepAliveMs,
    onTerminate: () => unsubscribe(),
  });
  const listener: SessionEventListener = (events: ReadonlyArray<Event>) => {
    channel.push(encodeSse(events));
  };

  const subscribed = await run(Effect.either(plane.subscribe(id, listener, agentId)));
  if (Either.isLeft(subscribed)) {
    channel.close();
    return errorResponse(subscribed.left, cors);
  }
  unsubscribe = subscribed.right;

  const stream = new ReadableStream<Uint8Array>(
    {
      start(controller) {
        channel.start(controller);
        const close = () => {
          channel.close();
          try {
            controller.close();
          } catch {
            // Already closed by the consumer.
          }
        };
        if (signal.aborted) close();
        else signal.addEventListener("abort", close, { once: true });
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

const HISTORY_ROLES: ReadonlySet<string> = new Set(["system", "user", "assistant", "tool"]);

/** Guard for the `blocks` field of an imported history item — malformed entries drop, not reject. */
const isHistoryBlock = Schema.is(Block);

/** Prompt payload caps — roughly the web composer's 5MB budget after base64 inflation. */
const MAX_PROMPT_PARTS = 16;
const MAX_PROMPT_PART_CHARS = 8 * 1024 * 1024;

const nonEmptyString = (value: unknown): value is string =>
  typeof value === "string" && value !== "";

const optionalString = (value: unknown): value is string | undefined =>
  value === undefined || typeof value === "string";

const payloadChars = (part: PromptPart): number => {
  switch (part.type) {
    case "text":
      return part.text.length;
    case "image":
    case "audio":
      return part.data.length;
    case "resource":
      return ("text" in part.resource ? part.resource.text : part.resource.blob).length;
    default:
      return 0;
  }
};

/**
 * Structural guard for one ACP `session/prompt` content block — the
 * `PromptPart` union (`text`, `image`, `audio`, `resource`, `resource_link`).
 * Anything else rejects the whole request: a silently dropped part would
 * make the agent see a prompt the user didn't send.
 */
const isPromptPart = (value: unknown): value is PromptPart => {
  if (!isRecord(value) || typeof value.type !== "string") return false;
  switch (value.type) {
    case "text":
      return typeof value.text === "string";
    case "image":
    case "audio":
      return nonEmptyString(value.data) && nonEmptyString(value.mimeType);
    case "resource": {
      const resource = value.resource;
      if (!isRecord(resource) || !nonEmptyString(resource.uri)) return false;
      const hasText = typeof resource.text === "string";
      const hasBlob = typeof resource.blob === "string";
      return (hasText || hasBlob) && optionalString(resource.mimeType);
    }
    case "resource_link":
      return (
        nonEmptyString(value.uri) &&
        nonEmptyString(value.name) &&
        optionalString(value.mimeType) &&
        (value.size === undefined || (typeof value.size === "number" && value.size >= 0))
      );
    default:
      return false;
  }
};

/**
 * `{text, attachments?}` → the content-block list handed to the agent: the
 * text part first, attachments in send order. Returns an error string when
 * the shape or budget is off.
 */
const promptPartsFromBody = (body: unknown): PromptPart[] | string => {
  if (!isRecord(body)) return "Expected a JSON object body";
  if (body.text !== undefined && typeof body.text !== "string") {
    return "text must be a string";
  }
  const text = typeof body.text === "string" ? body.text : "";
  const attachments = body.attachments;
  if (attachments !== undefined && !Array.isArray(attachments)) {
    return "attachments must be an array of content blocks";
  }
  const list = (attachments ?? []) as ReadonlyArray<unknown>;
  if (list.length > MAX_PROMPT_PARTS) {
    return `Too many attachments — max ${MAX_PROMPT_PARTS}`;
  }
  const parts: PromptPart[] = [];
  if (text.trim() !== "") parts.push({ type: "text", text });
  let chars = 0;
  for (const item of list) {
    if (!isPromptPart(item)) {
      return "attachments must be ACP content blocks (text, image, audio, resource, resource_link)";
    }
    chars += payloadChars(item);
    if (chars > MAX_PROMPT_PART_CHARS) {
      return "Attachments exceed the size limit";
    }
    parts.push(item);
  }
  if (parts.length === 0) return "text or attachments is required";
  return parts;
};

export interface SessionsRouteDeps {
  readonly run: EffectRunner;
  readonly plane: ControlPlaneService;
  readonly feed: NodeEventFeed;
  /** The instrumented meta overlay (`undefined` → meta routes 501). */
  readonly metaStore: MetaStore | undefined;
  readonly node: NodeIdentity;
  readonly keepAliveMs: number;
  /** Enables `POST /api/sessions/:id/convert` — the stores it converts between. */
  readonly convert: { readonly dbPath: string; readonly clineDir: string } | undefined;
  /** Test seam for `POST /api/sessions/import` (see AppOptions.importSession). */
  readonly importSession:
    | ((session: Session, agent: "cline" | "devin") => Effect.Effect<string, unknown>)
    | undefined;
  /** Title cache shared with the app-level live listener (push notifications). */
  readonly sessionTitles: Map<string, string>;
  readonly registerLiveListener: (id: string, agentId?: string) => void;
  readonly watchHeld: (id: string, agent?: string) => void;
  readonly unwatchHeld: (id: string, agent?: string) => void;
}

/**
 * `/api/sessions*` — the whole session surface: collection list/create,
 * `import`, the meta `PATCH`, `convert`, `DELETE`, and the `:id/:action`
 * verbs (history, checkpoints, export, stream, attach, prompt, restore,
 * rewind, cancel, permission). Internal check order mirrors the original
 * if-chain in app.ts.
 */
export const createSessionsRoute =
  (deps: SessionsRouteDeps): RouteHandler =>
  async ({ request, url, method, segments, cors, agentParam }) => {
    const { run, plane, feed, metaStore, node, keepAliveMs } = deps;

    if (method === "GET" && segmentsEqual(segments, ["api", "sessions"])) {
      const withLocks = url.searchParams.get("withLocks") === "1";
      return respond(run, plane.listSessions({ withLocks }), cors, {
        shape: (sessions) => {
          const overlaid = sessions.map((session) => {
            deps.sessionTitles.set(session.id, session.title);
            const meta = metaStore?.of(session.id);
            return {
              ...session,
              title: meta?.title ?? session.title,
              pinned: meta?.pinned ?? false,
              archived: meta?.archived ?? false,
              projectIds: meta?.projectIds ?? [],
              model: meta?.model ?? null,
              spans: meta?.spans ?? [],
            };
          });
          // Sessions created via POST /api/sessions but not yet flushed into
          // the agent's store survive restarts only in the meta file —
          // surface them so they stay reachable.
          const known = new Set(sessions.map((session) => session.id));
          const pending = Object.entries(metaStore?.sessions() ?? {}).flatMap(([id, meta]) =>
            known.has(id) || typeof meta.agent !== "string" || typeof meta.cwd !== "string"
              ? []
              : [
                  {
                    id,
                    title: meta.title ?? "New session",
                    cwd: meta.cwd,
                    agent: meta.agent,
                    updatedAt: meta.createdAt ?? new Date().toISOString(),
                    locked: false,
                    lockHolderPid: null,
                    source: "sepia",
                    busy: false,
                    pinned: meta.pinned ?? false,
                    archived: meta.archived ?? false,
                    projectIds: meta.projectIds ?? [],
                    model: meta.model ?? null,
                    spans: meta.spans ?? [],
                  },
                ],
          );
          return { sessions: [...overlaid, ...pending] };
        },
        span: "http.get /api/sessions",
      });
    }

    if (method === "POST" && segmentsEqual(segments, ["api", "sessions"])) {
      let body: unknown;
      try {
        body = await readJsonBody(request);
      } catch {
        return jsonResponse({ error: "Invalid JSON body" }, 400, cors);
      }
      if (!isRecord(body)) {
        return jsonResponse({ error: "Expected a JSON object body" }, 400, cors);
      }
      const cwd = body.cwd;
      if (typeof cwd !== "string" || cwd.trim() === "") {
        return jsonResponse({ error: "cwd is required" }, 400, cors);
      }
      const agentId = body.agent;
      if (agentId !== undefined && typeof agentId !== "string") {
        return jsonResponse({ error: "agent must be a string" }, 400, cors);
      }
      const title = body.title;
      if (title !== undefined && typeof title !== "string") {
        return jsonResponse({ error: "title must be a string" }, 400, cors);
      }
      const model = body.model;
      if (model !== undefined && typeof model !== "string") {
        return jsonResponse({ error: "model must be a string" }, 400, cors);
      }
      const fallbacks = body.fallbacks;
      if (
        fallbacks !== undefined &&
        !(Array.isArray(fallbacks) && fallbacks.every((f) => typeof f === "string"))
      ) {
        return jsonResponse({ error: "fallbacks must be an array of strings" }, 400, cors);
      }
      const created = plane
        .createSession({
          cwd,
          agentId,
          title,
          model,
          fallbacks: fallbacks as string[] | undefined,
        })
        .pipe(
          Effect.tap(({ id, agentId: createdAgent }) =>
            Effect.sync(() => {
              feed.emit(
                "session",
                sessionPayload(id, createdAgent, {
                  created: true,
                  cwd,
                  ...(title !== undefined ? { title } : {}),
                }),
              );
              // The agent may not flush the session to its store until the
              // first prompt; keep enough meta to identify it after a restart.
              deps.registerLiveListener(id, createdAgent);
              metaStore?.patch(id, {
                agent: createdAgent,
                cwd,
                createdAt: new Date().toISOString(),
                ...(model !== undefined ? { model } : {}),
                ...(title !== undefined ? { title } : {}),
              });
            }),
          ),
        );
      return respond(run, created, cors, {
        status: 201,
        span: "http.post /api/sessions",
      });
    }

    // Convert-with-explicit-IR: writes a session fetched from a peer node
    // into one of this node's agent stores (docs/protocol.md "Resume on…").
    // `{session}` carries the full IR verbatim (GET .../export); `{history}`
    // is the flat compat form for sources too old to serve it.
    if (method === "POST" && segmentsEqual(segments, ["api", "sessions", "import"])) {
      const conv = deps.convert;
      if (conv === undefined) {
        return jsonResponse({ error: "Import is not configured on this server" }, 501, cors);
      }
      let body: unknown;
      try {
        body = await readJsonBody(request);
      } catch {
        return jsonResponse({ error: "Invalid JSON body" }, 400, cors);
      }
      if (!isRecord(body)) {
        return jsonResponse({ error: "Expected a JSON object body" }, 400, cors);
      }
      const target = body.agent;
      if (target !== "cline" && target !== "devin") {
        return jsonResponse({ error: "agent must be 'cline' or 'devin'" }, 400, cors);
      }
      const cwd = body.cwd;
      if (cwd !== undefined && (typeof cwd !== "string" || cwd.trim() === "")) {
        return jsonResponse({ error: "cwd must be a non-empty string" }, 400, cors);
      }
      const title = body.title;
      if (title !== undefined && typeof title !== "string") {
        return jsonResponse({ error: "title must be a string" }, 400, cors);
      }
      const model = body.model;
      if (model !== undefined && typeof model !== "string") {
        return jsonResponse({ error: "model must be a string" }, 400, cors);
      }
      let session: Session;
      if (body.session !== undefined) {
        // Full-IR form: the `session` payload of GET /api/sessions/:id/export,
        // decoded verbatim — tool-call ids/args, thinking (+ signature),
        // per-node usage and the parent-linked tree all survive, where the
        // flat history form below keeps only the flat fields. A fresh id
        // keeps import semantics: every call lands as a new copy in the
        // target store.
        let decoded: Session;
        try {
          decoded = Conversion.sessionFromJson(body.session);
        } catch {
          return jsonResponse(
            { error: "session must be a session IR object (GET /api/sessions/:id/export)" },
            400,
            cors,
          );
        }
        session = Session.make({
          id: randomUUID(),
          title: title ?? decoded.title,
          workingDirectory: typeof cwd === "string" ? cwd.trim() : decoded.workingDirectory,
          backendType: decoded.backendType,
          agentMode: decoded.agentMode,
          model: model ?? decoded.model,
          createdAt: decoded.createdAt,
          lastActivityAt: decoded.lastActivityAt,
          mainChainId: decoded.mainChainId,
          shellLastSeenIndex: decoded.shellLastSeenIndex,
          cogsJson: decoded.cogsJson,
          workspaceDirs: decoded.workspaceDirs,
          hidden: decoded.hidden,
          parentSessionId: decoded.parentSessionId,
          agentId: decoded.agentId,
          checkpoints: decoded.checkpoints,
          metadata: decoded.metadata,
          nodes: decoded.nodes,
          promptHistory: decoded.promptHistory,
        });
      } else {
        const history = body.history;
        if (!Array.isArray(history) || history.length === 0) {
          return jsonResponse(
            { error: "import requires a session IR object or a non-empty history array" },
            400,
            cors,
          );
        }
        const messages: Conversion.ImportedHistoryMessage[] = [];
        for (const item of history) {
          if (
            !isRecord(item) ||
            typeof item.role !== "string" ||
            !HISTORY_ROLES.has(item.role) ||
            typeof item.content !== "string" ||
            typeof item.createdAt !== "number" ||
            !Number.isFinite(item.createdAt) ||
            (item.toolName !== undefined && typeof item.toolName !== "string")
          ) {
            return jsonResponse(
              { error: "history items must be { role, content, createdAt, toolName? } messages" },
              400,
              cors,
            );
          }
          messages.push({
            role: item.role as Conversion.ImportedHistoryMessage["role"],
            content: item.content,
            createdAt: item.createdAt,
            ...(typeof item.toolName === "string" ? { toolName: item.toolName } : {}),
            ...(typeof item.thinking === "string" ? { thinking: item.thinking } : {}),
            ...(typeof item.thinkingSignature === "string"
              ? { thinkingSignature: item.thinkingSignature }
              : {}),
            // IR v2 fields ride through when present so a converted session
            // keeps its metrics; anything malformed is dropped, not rejected.
            ...(isRecord(item.usage) &&
            typeof item.usage.input === "number" &&
            typeof item.usage.output === "number"
              ? { usage: item.usage as Conversion.ImportedHistoryMessage["usage"] }
              : {}),
            ...(typeof item.model === "string" ? { model: item.model } : {}),
            ...(typeof item.requestId === "string" ? { requestId: item.requestId } : {}),
            ...(typeof item.finishReason === "string" ? { finishReason: item.finishReason } : {}),
            ...(Array.isArray(item.blocks) ? { blocks: item.blocks.filter(isHistoryBlock) } : {}),
            ...(item.toolStatus === "pending" ||
            item.toolStatus === "success" ||
            item.toolStatus === "error"
              ? { toolStatus: item.toolStatus }
              : {}),
            ...(typeof item.exitCode === "number" && Number.isFinite(item.exitCode)
              ? { exitCode: item.exitCode }
              : {}),
            ...(typeof item.durationMs === "number" && Number.isFinite(item.durationMs)
              ? { durationMs: item.durationMs }
              : {}),
          });
        }

        const firstUser = messages.find((message) => message.role === "user");
        session = Conversion.sessionFromHistory({
          id: randomUUID(),
          title:
            title ??
            (firstUser !== undefined ? firstUser.content.slice(0, 80) : "Imported session"),
          cwd: typeof cwd === "string" ? cwd.trim() : process.cwd(),
          model: model ?? "sepia-import",
          history: messages,
        });
      }

      const executor = deps.importSession ?? defaultImportSession(conv);
      const importedAt = Date.now();
      const runSpan = { at: importedAt, agent: target, node: node.id };
      const asControl = executor(session, target).pipe(
        Effect.tap((sessionId) =>
          Effect.sync(() => {
            feed.emit("session", sessionPayload(sessionId, target, { created: true }));
            // Provenance: the imported copy's run continues under `target` on
            // this node — same record an attach would write.
            metaStore?.addSpan(sessionId, runSpan);
          }),
        ),
        Effect.mapError(
          (error) =>
            new ControlError({
              code: "internal",
              message: errorMessage(error),
              cause: error,
            }),
        ),
      );
      return respond(run, asControl, cors, {
        status: 201,
        shape: (sessionId) => ({
          id: sessionId,
          title: session.title,
          cwd: session.workingDirectory,
          agent: target,
          updatedAt: new Date(session.lastActivityAt * 1000).toISOString(),
          locked: false,
          lockHolderPid: null,
          source: target,
          busy: false,
          spans: metaStore === undefined ? [] : [runSpan],
        }),
        span: "http.post /api/sessions/import",
      });
    }

    if (
      method === "PATCH" &&
      segments[0] === "api" &&
      segments[1] === "sessions" &&
      segments.length === 3
    ) {
      const meta = metaStore;
      if (meta === undefined) {
        return jsonResponse({ error: "Rename is not configured on this server" }, 501, cors);
      }
      const id = decodeURIComponent(segments[2] ?? "");
      let patchBody: unknown;
      try {
        patchBody = await readJsonBody(request);
      } catch {
        return jsonResponse({ error: "Invalid JSON body" }, 400, cors);
      }
      if (!isRecord(patchBody)) {
        return jsonResponse({ error: "Expected a JSON object body" }, 400, cors);
      }
      const patch: Record<string, unknown> = {};
      if ("title" in patchBody) {
        const title = patchBody.title;
        if (typeof title !== "string" || title.trim() === "" || title.length > 200) {
          return jsonResponse({ error: "title must be a non-empty string (max 200)" }, 400, cors);
        }
        patch.title = title.trim();
      }
      if ("pinned" in patchBody) {
        if (typeof patchBody.pinned !== "boolean") {
          return jsonResponse({ error: "pinned must be a boolean" }, 400, cors);
        }
        patch.pinned = patchBody.pinned;
      }
      if ("archived" in patchBody) {
        if (typeof patchBody.archived !== "boolean") {
          return jsonResponse({ error: "archived must be a boolean" }, 400, cors);
        }
        patch.archived = patchBody.archived;
      }
      if ("projectIds" in patchBody) {
        const projectIds = patchBody.projectIds;
        if (!Array.isArray(projectIds) || !projectIds.every((p) => typeof p === "string")) {
          return jsonResponse({ error: "projectIds must be an array of strings" }, 400, cors);
        }
        patch.projectIds = projectIds;
      }
      if ("model" in patchBody) {
        const model = patchBody.model;
        if (model !== null && (typeof model !== "string" || model.length > 100)) {
          return jsonResponse({ error: "model must be a string or null" }, 400, cors);
        }
        patch.model = model;
      }
      if (Object.keys(patch).length === 0) {
        return jsonResponse({ error: "Nothing to patch" }, 400, cors);
      }
      meta.patch(id, patch);
      return jsonResponse({ ok: true }, 200, cors);
    }

    if (
      method === "POST" &&
      segments[0] === "api" &&
      segments[1] === "sessions" &&
      segments[3] === "convert" &&
      segments.length === 4
    ) {
      const conv = deps.convert;
      if (conv === undefined) {
        return jsonResponse({ error: "Convert is not configured on this server" }, 501, cors);
      }
      const id = decodeURIComponent(segments[2] ?? "");
      let body: unknown;
      try {
        body = await readJsonBody(request);
      } catch {
        return jsonResponse({ error: "Invalid JSON body" }, 400, cors);
      }
      const target = isRecord(body) ? body.agent : undefined;
      const fsLayer = Layer.mergeAll(BunFileSystem.layer, BunPath.layer);
      let effect: Effect.Effect<string, unknown>;
      if (target === "cline") {
        effect = Conversion.installCline(id, conv.clineDir).pipe(
          Effect.provide(
            Layer.mergeAll(
              Layer.provideMerge(
                ClineStore.layer(openSessionsDb, conv.clineDir),
                SqliteStorage.layerReadonly(conv.dbPath),
              ),
              fsLayer,
            ),
          ),
        );
      } else if (target === "devin") {
        effect = Conversion.importCline(join(conv.clineDir, "sessions", id)).pipe(
          Effect.provide(Layer.mergeAll(SqliteStorage.layer(conv.dbPath), fsLayer)),
        );
      } else {
        return jsonResponse({ error: "agent must be 'cline' or 'devin'" }, 400, cors);
      }
      const asControl = effect.pipe(
        Effect.tap((sessionId) =>
          Effect.sync(() => {
            feed.emit("session", sessionPayload(sessionId, target, { created: true }));
          }),
        ),
        Effect.mapError(
          (error) =>
            new ControlError({
              code: "internal",
              message: errorMessage(error),
              cause: error,
            }),
        ),
      );
      return respond(run, asControl, cors, {
        shape: (sessionId) => ({ sessionId }),
        span: "http.post /api/sessions/:id/convert",
      });
    }

    if (
      method === "DELETE" &&
      segments[0] === "api" &&
      segments[1] === "sessions" &&
      segments.length === 3
    ) {
      const id = decodeURIComponent(segments[2] ?? "");
      const deletion = plane.deleteSession(id, { agentId: agentParam }).pipe(
        // Created-but-unflushed sessions aren't in the repo — deleting them is
        // still a success: the meta record below is all that references them.
        Effect.catchIf(
          (error) => error.code === "not_found",
          () => Effect.void,
        ),
        Effect.tap(() =>
          Effect.sync(() => {
            deps.unwatchHeld(id, agentParam);
            feed.emit("session", sessionPayload(id, agentParam, { deleted: true }));
            metaStore?.remove(id);
          }),
        ),
      );
      return respond(run, deletion, cors, {
        shape: () => ({ ok: true }),
        span: "http.delete /api/sessions/:id",
      });
    }

    if (segments[0] === "api" && segments[1] === "sessions" && segments.length === 4) {
      const id = decodeURIComponent(segments[2] ?? "");
      const action = segments[3];

      if (method === "GET" && action === "history") {
        const rawLimit = url.searchParams.get("limit");
        let limit: number | undefined;
        if (rawLimit !== null && rawLimit !== "") {
          const value = Number(rawLimit);
          if (!Number.isInteger(value) || value < 0) {
            return jsonResponse({ error: "limit must be a non-negative integer" }, 400, cors);
          }
          limit = value;
        }
        const rawBefore = url.searchParams.get("before");
        let before: number | undefined;
        if (rawBefore !== null && rawBefore !== "") {
          const value = Number(rawBefore);
          if (!Number.isInteger(value) || value < 0) {
            return jsonResponse({ error: "before must be a non-negative integer" }, 400, cors);
          }
          before = value;
        }
        const historyOptions =
          limit === undefined && before === undefined && agentParam === undefined
            ? undefined
            : { limit, before, agentId: agentParam };
        return respond(run, plane.getHistory(id, historyOptions), cors, {
          span: "http.get /api/sessions/:id/history",
        });
      }

      if (method === "GET" && action === "checkpoints") {
        // The workspace-snapshot refs the store recorded (Cline shadow-git
        // `metadata.checkpoint` history, Claude `file-history-snapshot`
        // entries). Cheap sibling of /export for the restore UI — just the
        // refs, never the payloads.
        return respond(run, plane.getSummary(id, { agentId: agentParam }), cors, {
          shape: (session) => ({ checkpoints: session.checkpoints }),
          span: "http.get /api/sessions/:id/checkpoints",
        });
      }

      if (method === "GET" && action === "export") {
        // The unprojected sibling of /history: the complete session IR —
        // nodes with toolCalls ids/args, thinking, usage and parent links —
        // that a peer node's /import consumes for a lossless cross-node
        // resume. Older nodes 404 here, which is exactly the client's cue to
        // fall back to paged /history.
        return respond(run, plane.getSession(id, { agentId: agentParam }), cors, {
          shape: (session) => ({ session: Conversion.sessionToJson(session) }),
          span: "http.get /api/sessions/:id/export",
        });
      }

      if (method === "GET" && action === "stream") {
        return streamResponse(run, plane, id, request.signal, cors, keepAliveMs, agentParam);
      }

      if (method === "POST" && action === "attach") {
        let takeover = false;
        let model: string | undefined;
        let fallbacks: ReadonlyArray<string> | undefined;
        try {
          const body = await readJsonBody(request);
          if (body !== undefined) {
            if (!isRecord(body)) {
              return jsonResponse({ error: "Expected a JSON object body" }, 400, cors);
            }
            takeover = body.takeover === true;
            if (typeof body.model === "string") model = body.model;
            if (
              Array.isArray(body.fallbacks) &&
              body.fallbacks.every((f: unknown) => typeof f === "string")
            ) {
              fallbacks = body.fallbacks as string[];
            }
          }
        } catch {
          return jsonResponse({ error: "Invalid JSON body" }, 400, cors);
        }
        return respond(
          run,
          plane.attach(id, { takeover, model, fallbacks, agentId: agentParam }).pipe(
            Effect.tap((result) =>
              Effect.sync(() => {
                deps.registerLiveListener(id, agentParam);
                feed.emit("session", sessionPayload(id, result.agentId, { live: result.attached }));
                // Read-only means another process holds the store lock —
                // the watch turns its release (and its transcript writes)
                // into feed events so held clients never poll for it.
                if (result.readOnly) deps.watchHeld(id, result.agentId);
                else if (result.attached) deps.unwatchHeld(id, result.agentId);
                // Provenance: an attach means the run continues under this
                // node's control plane — record which agent + node own the
                // span. Idempotent, so a same-agent re-attach doesn't dup.
                if (result.attached) {
                  metaStore?.addSpan(id, {
                    at: Date.now(),
                    agent: result.agentId,
                    node: node.id,
                  });
                }
              }),
            ),
            // A failed takeover leaves the session held — keep the watch so
            // the release edge still reaches the feed.
            Effect.tapError((error) =>
              Effect.sync(() => {
                if (error.code === "locked") deps.watchHeld(id, agentParam);
              }),
            ),
          ),
          cors,
          {
            span: "http.post /api/sessions/:id/attach",
          },
        );
      }

      if (method === "POST" && action === "prompt") {
        let body: unknown;
        try {
          body = await readJsonBody(request);
        } catch {
          return jsonResponse({ error: "Invalid JSON body" }, 400, cors);
        }
        const parts = promptPartsFromBody(body);
        if (typeof parts === "string") {
          return jsonResponse({ error: parts }, 400, cors);
        }
        return respond(run, plane.prompt(id, parts, agentParam), cors, {
          shape: () => ({ ok: true }),
          span: "http.post /api/sessions/:id/prompt",
        });
      }

      if (method === "POST" && action === "restore") {
        // File restore — writes/deletes real files under the session's cwd.
        // `{path, toolCallId?}` reverts recorded diffs; `{checkpoint, paths?}`
        // materializes a recorded shadow-git or file-history-snapshot ref.
        // `confirm: true` required.
        let body: unknown;
        try {
          body = await readJsonBody(request);
        } catch {
          return jsonResponse({ error: "Invalid JSON body" }, 400, cors);
        }
        if (!isRecord(body)) {
          return jsonResponse({ error: "Expected a JSON object body" }, 400, cors);
        }
        if (body.confirm !== true) {
          return jsonResponse({ error: "Restore requires confirm: true" }, 400, cors);
        }
        if (body.path !== undefined && typeof body.path !== "string") {
          return jsonResponse({ error: "path must be a string" }, 400, cors);
        }
        if (body.toolCallId !== undefined && typeof body.toolCallId !== "string") {
          return jsonResponse({ error: "toolCallId must be a string" }, 400, cors);
        }
        if (body.checkpoint !== undefined && typeof body.checkpoint !== "string") {
          return jsonResponse({ error: "checkpoint must be a string" }, 400, cors);
        }
        if (
          body.paths !== undefined &&
          !(Array.isArray(body.paths) && body.paths.every((p) => typeof p === "string"))
        ) {
          return jsonResponse({ error: "paths must be an array of strings" }, 400, cors);
        }
        return respond(
          run,
          plane.restore(
            id,
            {
              confirm: true,
              path: body.path as string | undefined,
              toolCallId: body.toolCallId as string | undefined,
              checkpoint: body.checkpoint as string | undefined,
              paths: body.paths as string[] | undefined,
            },
            agentParam,
          ),
          cors,
          { span: "http.post /api/sessions/:id/restore" },
        );
      }

      if (method === "POST" && action === "rewind") {
        // Conversation rewind — truncates the transcript at a point:
        // `{nodeId}` keeps that node and everything before it, `{turns: n}`
        // drops the last n user turns, `{checkpoint}` rewinds to a recorded
        // snapshot ref. `confirm: true` required; refused while the session
        // is busy or held by a live process.
        let body: unknown;
        try {
          body = await readJsonBody(request);
        } catch {
          return jsonResponse({ error: "Invalid JSON body" }, 400, cors);
        }
        if (!isRecord(body)) {
          return jsonResponse({ error: "Expected a JSON object body" }, 400, cors);
        }
        if (body.confirm !== true) {
          return jsonResponse({ error: "Rewind requires confirm: true" }, 400, cors);
        }
        const nodeId = body.nodeId;
        if (
          nodeId !== undefined &&
          (typeof nodeId !== "number" || !Number.isInteger(nodeId) || nodeId < 0)
        ) {
          return jsonResponse({ error: "nodeId must be a non-negative integer" }, 400, cors);
        }
        const turns = body.turns;
        if (
          turns !== undefined &&
          (typeof turns !== "number" || !Number.isInteger(turns) || turns < 1)
        ) {
          return jsonResponse({ error: "turns must be a positive integer" }, 400, cors);
        }
        if (body.checkpoint !== undefined && typeof body.checkpoint !== "string") {
          return jsonResponse({ error: "checkpoint must be a string" }, 400, cors);
        }
        return respond(
          run,
          plane
            .rewind(
              id,
              {
                confirm: true,
                nodeId: nodeId as number | undefined,
                turns: turns as number | undefined,
                checkpoint: body.checkpoint as string | undefined,
              },
              agentParam,
            )
            .pipe(
              Effect.tap(() =>
                Effect.sync(() => {
                  feed.emit("session", sessionPayload(id, agentParam, { rewound: true }));
                }),
              ),
            ),
          cors,
          { span: "http.post /api/sessions/:id/rewind" },
        );
      }

      if (method === "POST" && action === "cancel") {
        return respond(run, plane.cancel(id, agentParam), cors, {
          shape: () => ({ ok: true }),
          span: "http.post /api/sessions/:id/cancel",
        });
      }

      if (method === "POST" && action === "permission") {
        let body: unknown;
        try {
          body = await readJsonBody(request);
        } catch {
          return jsonResponse({ error: "Invalid JSON body" }, 400, cors);
        }
        if (!isRecord(body) || typeof body.requestId !== "string") {
          return jsonResponse({ error: "requestId is required" }, 400, cors);
        }
        const optionId = body.optionId === undefined ? null : body.optionId;
        if (optionId !== null && typeof optionId !== "string") {
          return jsonResponse({ error: "optionId must be a string or null" }, 400, cors);
        }
        return respond(
          run,
          plane.respondToPermission(id, body.requestId, optionId, agentParam),
          cors,
          {
            span: "http.post /api/sessions/:id/permission",
            shape: () => ({ ok: true }),
          },
        );
      }
    }

    return undefined;
  };

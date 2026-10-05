import { Args, Command, Options } from "@effect/cli";
import * as Fs from "@effect/platform/FileSystem";
import { Console, Effect, Option } from "effect";
import type { HistoryMessage, SessionSummary } from "sepia-session-control";
import * as api from "./api-client";
import { defaultNodeUrl, resolveTarget, type NodeTarget } from "./api-client";

/**
 * The node-op verbs — every command here talks to a running sepia node's
 * REST API (docs/protocol.md). Each takes `--node`/`--token` (defaults:
 * `SEPIA_NODE_URL` → http://127.0.0.1:8787, `SEPIA_TOKEN` → none). Store ops
 * that work on local agent stores directly live in main.ts.
 */

const nodeOption = Options.text("node").pipe(
  Options.withDefault(defaultNodeUrl()),
  Options.withDescription(
    "Base URL of the sepia node (default: SEPIA_NODE_URL, else http://127.0.0.1:8787)",
  ),
);

const tokenOption = Options.text("token").pipe(
  Options.optional,
  Options.withDescription("Bearer token for the node (default: SEPIA_TOKEN)"),
);

const jsonOption = Options.boolean("json").pipe(
  Options.withDefault(false),
  Options.withDescription("Print the raw API response as JSON"),
);

const agentQueryOption = Options.text("agent").pipe(
  Options.optional,
  Options.withDescription(
    "Scope the session id lookup to this agent's store — ids collide across agents",
  ),
);

const target = (node: string, token: Option.Option<string>): NodeTarget =>
  resolveTarget(node, Option.getOrUndefined(token));

const printJson = (value: unknown) => Console.log(JSON.stringify(value, null, 2));

const rowFlags = (session: SessionSummary): string =>
  [
    session.locked ? "locked" : "",
    session.busy ? "busy" : "",
    session.pinned === true ? "pinned" : "",
    session.archived === true ? "archived" : "",
  ]
    .filter((flag) => flag !== "")
    .join(",");

const printSession = (session: SessionSummary) => {
  const flags = rowFlags(session);
  return Console.log(
    `${session.agent}:${session.id}\t${session.title}\t${session.cwd}\t${session.updatedAt}${flags === "" ? "" : `\t${flags}`}`,
  );
};

/** SSE frames land here — a named frame prints `kind {json}`, an unnamed one just its data line. */
const printFrame = (frame: api.SseFrame): void => {
  console.log(frame.event === undefined ? frame.data : `${frame.event} ${frame.data}`);
};

const sessionIdArg = Args.text({ name: "session-id" }).pipe(
  Args.withDescription("Session id on the target node"),
);

// --- Top-level node verbs ----------------------------------------------------

const healthCommand = Command.make(
  "health",
  { node: nodeOption, token: tokenOption, json: jsonOption },
  ({ node, token, json }) =>
    Effect.gen(function* () {
      const health = yield* api.getHealth(target(node, token));
      if (json) return yield* printJson(health);
      yield* Console.log(health.ok ? "ok" : "unhealthy");
    }),
).pipe(Command.withDescription("GET /api/health — is the node up and its store readable"));

const nodeCommand = Command.make(
  "node",
  { node: nodeOption, token: tokenOption, json: jsonOption },
  ({ node, token, json }) =>
    Effect.gen(function* () {
      const descriptor = yield* api.getNode(target(node, token));
      if (json) return yield* printJson(descriptor);
      yield* Console.log(`${descriptor.name} (${descriptor.id})`);
      yield* Console.log(`  version:      ${descriptor.version}`);
      yield* Console.log(`  protocol:     ${descriptor.protocol}`);
      yield* Console.log(`  agents:       ${descriptor.agents.join(", ") || "none"}`);
      yield* Console.log(`  capabilities: ${descriptor.capabilities.join(", ")}`);
    }),
).pipe(Command.withDescription("GET /api/node — the node's identity, agents and capabilities"));

const agentsCommand = Command.make(
  "agents",
  { node: nodeOption, token: tokenOption, json: jsonOption },
  ({ node, token, json }) =>
    Effect.gen(function* () {
      const agents = yield* api.listAgents(target(node, token));
      if (json) return yield* printJson(agents);
      for (const agent of agents) {
        yield* Console.log(
          `${agent.id}\t${agent.label}${agent.capabilities === undefined ? "" : "\tcapabilities probed"}`,
        );
      }
    }),
).pipe(Command.withDescription("GET /api/agents — the agent runtimes registered on the node"));

const userCommand = Command.make(
  "user",
  { node: nodeOption, token: tokenOption, json: jsonOption },
  ({ node, token, json }) =>
    Effect.gen(function* () {
      const { user } = yield* api.getUser(target(node, token));
      if (json) return yield* printJson(user);
      yield* Console.log(
        `${user.username}@${user.hostname}\t${user.homedir}\t${user.shell ?? "-"}\t${user.platform}/${user.arch}`,
      );
    }),
).pipe(Command.withDescription("GET /api/user — the OS user the node runs as"));

const fsCommand = Command.make(
  "fs",
  {
    path: Args.text({ name: "path" }).pipe(
      Args.withDescription("Absolute directory path on the node"),
    ),
    node: nodeOption,
    token: tokenOption,
    json: jsonOption,
  },
  ({ path, node, token, json }) =>
    Effect.gen(function* () {
      const dirs = yield* api.listDirs(target(node, token), path);
      if (json) return yield* printJson({ dirs });
      for (const dir of dirs) yield* Console.log(dir);
    }),
).pipe(Command.withDescription("GET /api/fs — list subdirectories of a path on the node"));

const eventsCommand = Command.make(
  "events",
  { node: nodeOption, token: tokenOption },
  ({ node, token }) => api.streamSse(target(node, token), "/api/events", printFrame),
).pipe(
  Command.withDescription(
    "GET /api/events — stream the node event feed (session/meta/project/heartbeat) to stdout",
  ),
);

const redeemCommand = Command.make(
  "redeem",
  {
    code: Args.text({ name: "code" }).pipe(
      Args.withDescription("One-time code printed by `sepia pair` on the node"),
    ),
    node: nodeOption,
  },
  ({ code, node }) =>
    Effect.gen(function* () {
      const { token } = yield* api.pairRedeem(resolveTarget(node, undefined), code);
      yield* Console.log(token);
      yield* Console.error("Export it as SEPIA_TOKEN or pass it via --token.");
    }),
).pipe(
  Command.withDescription(
    "POST /api/pair — exchange a `sepia pair` code for a long-lived bearer token",
  ),
);

// --- sessions ----------------------------------------------------------------

const sessionsListCommand = Command.make(
  "list",
  {
    locks: Options.boolean("locks").pipe(
      Options.withDefault(false),
      Options.withDescription("Also probe each agent for live lock state (?withLocks=1)"),
    ),
    node: nodeOption,
    token: tokenOption,
    json: jsonOption,
  },
  ({ locks, node, token, json }) =>
    Effect.gen(function* () {
      const sessions = yield* api.listSessions(target(node, token), { withLocks: locks });
      if (json) return yield* printJson({ sessions });
      for (const session of sessions) yield* printSession(session);
    }),
).pipe(Command.withDescription("GET /api/sessions — list the node's sessions"));

const sessionsCreateCommand = Command.make(
  "create",
  {
    cwd: Options.text("cwd").pipe(
      Options.withDefault(process.cwd()),
      Options.withDescription("Working directory the agent session runs in"),
    ),
    agent: Options.text("agent").pipe(
      Options.optional,
      Options.withDescription("Agent runtime to spawn (default: the node's default agent)"),
    ),
    title: Options.text("title").pipe(Options.optional, Options.withDescription("Session title")),
    model: Options.text("model").pipe(Options.optional, Options.withDescription("Preferred model")),
    fallback: Options.text("fallback").pipe(
      Options.repeated,
      Options.withDescription("Fallback model — repeatable, applied in order"),
    ),
    node: nodeOption,
    token: tokenOption,
    json: jsonOption,
  },
  ({ cwd, agent, title, model, fallback, node, token, json }) =>
    Effect.gen(function* () {
      const created = yield* api.createSession(target(node, token), {
        cwd,
        agent: Option.getOrUndefined(agent),
        title: Option.getOrUndefined(title),
        model: Option.getOrUndefined(model),
        fallbacks: fallback.length === 0 ? undefined : fallback,
      });
      if (json) return yield* printJson(created);
      yield* Console.log(`Created session ${created.id} on agent ${created.agentId}`);
    }),
).pipe(Command.withDescription("POST /api/sessions — create a session"));

const sessionsAttachCommand = Command.make(
  "attach",
  {
    sessionId: sessionIdArg,
    takeover: Options.boolean("takeover").pipe(
      Options.withDefault(false),
      Options.withDescription("SIGTERM the lock holder and attach writable"),
    ),
    model: Options.text("model").pipe(
      Options.optional,
      Options.withDescription("Preferred model for this attach"),
    ),
    fallback: Options.text("fallback").pipe(
      Options.repeated,
      Options.withDescription("Fallback model — repeatable"),
    ),
    agent: agentQueryOption,
    node: nodeOption,
    token: tokenOption,
    json: jsonOption,
  },
  ({ sessionId, takeover, model, fallback, agent, node, token, json }) =>
    Effect.gen(function* () {
      const result = yield* api.attach(target(node, token), sessionId, {
        takeover,
        model: Option.getOrUndefined(model),
        fallbacks: fallback.length === 0 ? undefined : fallback,
        agent: Option.getOrUndefined(agent),
      });
      if (json) return yield* printJson(result);
      const mode = result.attached ? (result.readOnly ? "read-only" : "writable") : "not attached";
      yield* Console.log(`Session ${sessionId}: ${mode} (${result.agentId})`);
    }),
).pipe(
  Command.withDescription(
    "POST /api/sessions/:id/attach — attach live control (read-only while another process holds the lock)",
  ),
);

const promptEffect = (
  t: NodeTarget,
  sessionId: string,
  text: string,
  agent: Option.Option<string>,
) =>
  api
    .prompt(t, sessionId, text, undefined, Option.getOrUndefined(agent))
    .pipe(Effect.andThen(Console.log(`Prompt sent to session ${sessionId}`)));

const promptOptions = {
  sessionId: sessionIdArg,
  text: Args.text({ name: "text" }).pipe(Args.withDescription("The prompt text to send")),
  agent: agentQueryOption,
  node: nodeOption,
  token: tokenOption,
};

export const promptCommand = Command.make("prompt", promptOptions, (args) =>
  promptEffect(target(args.node, args.token), args.sessionId, args.text, args.agent),
).pipe(
  Command.withDescription("POST /api/sessions/:id/prompt — send one turn to an attached session"),
);

const sessionsCancelCommand = Command.make(
  "cancel",
  { sessionId: sessionIdArg, agent: agentQueryOption, node: nodeOption, token: tokenOption },
  ({ sessionId, agent, node, token }) =>
    api
      .cancel(target(node, token), sessionId, Option.getOrUndefined(agent))
      .pipe(Effect.andThen(Console.log(`Cancelled session ${sessionId}`))),
).pipe(Command.withDescription("POST /api/sessions/:id/cancel — stop the current run"));

const sessionsPermissionCommand = Command.make(
  "permission",
  {
    sessionId: sessionIdArg,
    request: Options.text("request").pipe(
      Options.withDescription("The pending permission request id (requestId)"),
    ),
    option: Options.text("option").pipe(
      Options.optional,
      Options.withDescription("The option id to accept; omit to decline the request"),
    ),
    agent: agentQueryOption,
    node: nodeOption,
    token: tokenOption,
  },
  ({ sessionId, request, option, agent, node, token }) =>
    api
      .respondToPermission(
        target(node, token),
        sessionId,
        request,
        Option.getOrElse(option, () => null),
        Option.getOrUndefined(agent),
      )
      .pipe(Effect.andThen(Console.log(`Answered permission ${request} on session ${sessionId}`))),
).pipe(
  Command.withDescription(
    "POST /api/sessions/:id/permission — answer a pending permission request",
  ),
);

const printHistoryMessage = (message: HistoryMessage) =>
  Effect.gen(function* () {
    const tool = message.toolName === undefined ? "" : `(${message.toolName})`;
    const at = new Date(message.createdAt).toISOString();
    yield* Console.log(`[${at}] ${message.role}${tool}: ${message.content}`);
  });

const sessionsHistoryCommand = Command.make(
  "history",
  {
    sessionId: sessionIdArg,
    limit: Options.integer("limit").pipe(
      Options.optional,
      Options.withDescription("Number of trailing messages to return"),
    ),
    before: Options.integer("before").pipe(
      Options.optional,
      Options.withDescription("Exclusive end index — page backwards through the backlog"),
    ),
    agent: agentQueryOption,
    node: nodeOption,
    token: tokenOption,
    json: jsonOption,
  },
  ({ sessionId, limit, before, agent, node, token, json }) =>
    Effect.gen(function* () {
      const page = yield* api.getHistory(target(node, token), sessionId, {
        limit: Option.getOrUndefined(limit),
        before: Option.getOrUndefined(before),
        agent: Option.getOrUndefined(agent),
      });
      if (json) return yield* printJson(page);
      for (const message of page.messages) yield* printHistoryMessage(message);
      if (page.start > 0) {
        yield* Console.error(`… ${page.start} earlier message(s); pass --before ${page.start}`);
      }
    }),
).pipe(Command.withDescription("GET /api/sessions/:id/history — the paginated backlog"));

const sessionsCheckpointsCommand = Command.make(
  "checkpoints",
  {
    sessionId: sessionIdArg,
    agent: agentQueryOption,
    node: nodeOption,
    token: tokenOption,
    json: jsonOption,
  },
  ({ sessionId, agent, node, token, json }) =>
    Effect.gen(function* () {
      const checkpoints = yield* api.getCheckpoints(
        target(node, token),
        sessionId,
        Option.getOrUndefined(agent),
      );
      if (json) return yield* printJson({ checkpoints });
      for (const checkpoint of checkpoints) {
        const kind = checkpoint.kind === undefined ? "" : `\t${checkpoint.kind}`;
        const runs = checkpoint.runCount === undefined ? "" : `\truns:${checkpoint.runCount}`;
        yield* Console.log(
          `${checkpoint.ref}\t${new Date(checkpoint.createdAt).toISOString()}${kind}${runs}`,
        );
      }
    }),
).pipe(
  Command.withDescription(
    "GET /api/sessions/:id/checkpoints — workspace snapshot refs the store recorded",
  ),
);

const sessionsExportCommand = Command.make(
  "export",
  {
    sessionId: sessionIdArg,
    out: Options.text("out").pipe(
      Options.optional,
      Options.withDescription("Write the session IR JSON to this file (default: stdout)"),
    ),
    agent: agentQueryOption,
    node: nodeOption,
    token: tokenOption,
  },
  ({ sessionId, out, agent, node, token }) =>
    Effect.gen(function* () {
      const session = yield* api.exportSession(
        target(node, token),
        sessionId,
        Option.getOrUndefined(agent),
      );
      const json = `${JSON.stringify(session, null, 2)}\n`;
      const outPath = Option.getOrUndefined(out);
      if (outPath === undefined || outPath === "-") return yield* Console.log(json.trimEnd());
      const fs = yield* Fs.FileSystem;
      yield* fs.writeFileString(outPath, json);
      yield* Console.log(`Exported session ${sessionId} to ${outPath}`);
    }),
).pipe(
  Command.withDescription(
    "GET /api/sessions/:id/export — the complete session IR (feeds `sessions import`)",
  ),
);

const sessionsStreamCommand = Command.make(
  "stream",
  {
    sessionId: sessionIdArg,
    agent: agentQueryOption,
    node: nodeOption,
    token: tokenOption,
  },
  ({ sessionId, agent, node, token }) => {
    const query = Option.match(agent, {
      onNone: () => "",
      onSome: (value) => `?agent=${encodeURIComponent(value)}`,
    });
    return api.streamSse(
      target(node, token),
      `/api/sessions/${encodeURIComponent(sessionId)}/stream${query}`,
      printFrame,
    );
  },
).pipe(
  Command.withDescription(
    "GET /api/sessions/:id/stream — stream the live run's AG-UI events to stdout",
  ),
);

const sessionsRunCommand = Command.make(
  "run",
  {
    sessionId: sessionIdArg,
    text: Args.text({ name: "text" }).pipe(Args.withDescription("The prompt text to send")),
    agent: agentQueryOption,
    node: nodeOption,
    token: tokenOption,
  },
  ({ sessionId, text, agent, node, token }) => {
    const params = new URLSearchParams({ sessionId });
    const agentValue = Option.getOrUndefined(agent);
    if (agentValue !== undefined) params.set("agent", agentValue);
    return api.streamSse(target(node, token), `/api/agent?${params.toString()}`, printFrame, {
      method: "POST",
      body: {
        threadId: sessionId,
        messages: [{ role: "user", content: text }],
      },
    });
  },
).pipe(
  Command.withDescription(
    "POST /api/agent — attach, send one prompt and stream the AG-UI run until it finishes",
  ),
);

const sessionsMetaCommand = Command.make(
  "meta",
  {
    sessionId: sessionIdArg,
    title: Options.text("title").pipe(
      Options.optional,
      Options.withDescription("Rename the session"),
    ),
    pin: Options.boolean("pin").pipe(
      Options.withDefault(false),
      Options.withDescription("Pin the session"),
    ),
    unpin: Options.boolean("unpin").pipe(
      Options.withDefault(false),
      Options.withDescription("Unpin the session"),
    ),
    archive: Options.boolean("archive").pipe(
      Options.withDefault(false),
      Options.withDescription("Archive the session"),
    ),
    unarchive: Options.boolean("unarchive").pipe(
      Options.withDefault(false),
      Options.withDescription("Unarchive the session"),
    ),
    project: Options.text("project").pipe(
      Options.repeated,
      Options.withDescription("Project ids to assign — repeatable; replaces the whole list"),
    ),
    model: Options.text("model").pipe(
      Options.optional,
      Options.withDescription("Recorded model override"),
    ),
    clearModel: Options.boolean("clear-model").pipe(
      Options.withDefault(false),
      Options.withDescription("Clear the recorded model override"),
    ),
    agent: agentQueryOption,
    node: nodeOption,
    token: tokenOption,
  },
  ({
    sessionId,
    title,
    pin,
    unpin,
    archive,
    unarchive,
    project,
    model,
    clearModel,
    agent,
    node,
    token,
  }) =>
    Effect.gen(function* () {
      if (pin && unpin) return yield* Effect.fail(new Error("--pin and --unpin conflict"));
      if (archive && unarchive) {
        return yield* Effect.fail(new Error("--archive and --unarchive conflict"));
      }
      if (Option.isSome(model) && clearModel) {
        return yield* Effect.fail(new Error("--model and --clear-model conflict"));
      }
      const patch: api.SessionMetaPatch = {
        ...(Option.isSome(title) ? { title: title.value } : {}),
        ...(pin ? { pinned: true } : {}),
        ...(unpin ? { pinned: false } : {}),
        ...(archive ? { archived: true } : {}),
        ...(unarchive ? { archived: false } : {}),
        ...(project.length > 0 ? { projectIds: project } : {}),
        ...(Option.isSome(model) ? { model: model.value } : {}),
        ...(clearModel ? { model: null } : {}),
      };
      if (Object.keys(patch).length === 0) {
        return yield* Effect.fail(new Error("Nothing to patch — pass a flag to change"));
      }
      yield* api.patchSession(target(node, token), sessionId, patch, Option.getOrUndefined(agent));
      yield* Console.log(`Updated session ${sessionId}`);
    }),
).pipe(
  Command.withDescription(
    "PATCH /api/sessions/:id — the meta overlay (title, pinned, archived, projects, model)",
  ),
);

const sessionsRenameCommand = Command.make(
  "rename",
  {
    sessionId: sessionIdArg,
    title: Args.text({ name: "title" }).pipe(Args.withDescription("The new title")),
    agent: agentQueryOption,
    node: nodeOption,
    token: tokenOption,
  },
  ({ sessionId, title, agent, node, token }) =>
    api
      .patchSession(target(node, token), sessionId, { title }, Option.getOrUndefined(agent))
      .pipe(Effect.andThen(Console.log(`Renamed session ${sessionId}`))),
).pipe(Command.withDescription("PATCH /api/sessions/:id — rename a session"));

const sessionsDeleteCommand = Command.make(
  "delete",
  { sessionId: sessionIdArg, agent: agentQueryOption, node: nodeOption, token: tokenOption },
  ({ sessionId, agent, node, token }) =>
    api
      .deleteSession(target(node, token), sessionId, Option.getOrUndefined(agent))
      .pipe(Effect.andThen(Console.log(`Deleted session ${sessionId}`))),
).pipe(Command.withDescription("DELETE /api/sessions/:id"));

const sessionsConvertCommand = Command.make(
  "convert",
  {
    sessionId: sessionIdArg,
    to: Options.choice("to", ["cline", "devin"]).pipe(
      Options.withDescription("Agent store to convert into"),
    ),
    node: nodeOption,
    token: tokenOption,
  },
  ({ sessionId, to, node, token }) =>
    Effect.gen(function* () {
      const { sessionId: converted } = yield* api.convertSession(
        target(node, token),
        sessionId,
        to,
      );
      yield* Console.log(`Converted session ${sessionId} → ${to}: ${converted}`);
    }),
).pipe(
  Command.withDescription(
    "POST /api/sessions/:id/convert — convert a session into another agent's store on the node",
  ),
);

const sessionsImportCommand = Command.make(
  "import",
  {
    path: Args.file({ name: "path" }).pipe(
      Args.withDescription(
        "A session JSON export (`sessions export` / `sepia export` output) or a history array",
      ),
    ),
    to: Options.choice("to", ["cline", "devin"]).pipe(
      Options.withDescription("Agent store to write into"),
    ),
    cwd: Options.text("cwd").pipe(
      Options.optional,
      Options.withDescription("Override the imported working directory"),
    ),
    title: Options.text("title").pipe(
      Options.optional,
      Options.withDescription("Override the imported title"),
    ),
    model: Options.text("model").pipe(
      Options.optional,
      Options.withDescription("Override the imported model"),
    ),
    node: nodeOption,
    token: tokenOption,
    json: jsonOption,
  },
  ({ path, to, cwd, title, model, node, token, json }) =>
    Effect.gen(function* () {
      const fs = yield* Fs.FileSystem;
      const raw = yield* fs
        .readFileString(path)
        .pipe(Effect.mapError((cause) => new Error(`Cannot read ${path}: ${String(cause)}`)));
      let parsed: unknown;
      try {
        parsed = JSON.parse(raw);
      } catch {
        return yield* Effect.fail(new Error(`${path} is not valid JSON`));
      }
      // A bare array is the flat history form; `{session: …}` is the /export
      // envelope; anything else is treated as the session IR itself.
      const body = Array.isArray(parsed)
        ? { history: parsed }
        : typeof parsed === "object" &&
            parsed !== null &&
            "session" in parsed &&
            (parsed as Record<string, unknown>).session !== undefined
          ? { session: (parsed as Record<string, unknown>).session }
          : { session: parsed };
      const summary = yield* api.importSession(target(node, token), {
        agent: to,
        ...body,
        cwd: Option.getOrUndefined(cwd),
        title: Option.getOrUndefined(title),
        model: Option.getOrUndefined(model),
      });
      if (json) return yield* printJson(summary);
      yield* printSession(summary);
    }),
).pipe(
  Command.withDescription(
    "POST /api/sessions/import — write session IR (or a history array) into an agent store",
  ),
);

const RESUME_PAGE_SIZE = 500;

const sessionsResumeCommand = Command.make(
  "resume",
  {
    sessionId: Args.text({ name: "session-id" }).pipe(
      Args.withDescription("Session id on the source node"),
    ),
    to: Options.choice("to", ["cline", "devin"]).pipe(
      Options.withDescription("Agent store on the target node to resume into"),
    ),
    source: Options.text("source").pipe(
      Options.withDefault(defaultNodeUrl()),
      Options.withDescription("Source node URL (default: same as --node)"),
    ),
    sourceToken: Options.text("source-token").pipe(
      Options.optional,
      Options.withDescription("Bearer token for the source node (default: --token)"),
    ),
    fromAgent: Options.text("from-agent").pipe(
      Options.optional,
      Options.withDescription("Scope the source lookup to this agent's store"),
    ),
    cwd: Options.text("cwd").pipe(
      Options.optional,
      Options.withDescription("Override the working directory on the target"),
    ),
    title: Options.text("title").pipe(
      Options.optional,
      Options.withDescription("Override the title on the target"),
    ),
    model: Options.text("model").pipe(
      Options.optional,
      Options.withDescription("Override the model on the target"),
    ),
    node: nodeOption,
    token: tokenOption,
    json: jsonOption,
  },
  ({ sessionId, to, source, sourceToken, fromAgent, cwd, title, model, node, token, json }) =>
    Effect.gen(function* () {
      const sourceTarget = resolveTarget(
        source,
        Option.getOrElse(sourceToken, () => Option.getOrUndefined(token)),
      );
      const fromAgentValue = Option.getOrUndefined(fromAgent);
      // Prefer the full IR from /export; a node too old to serve it 404s and
      // falls back to paging the flat /history projection — the compat path.
      const exported = yield* api.exportSession(sourceTarget, sessionId, fromAgentValue).pipe(
        Effect.catchIf(
          (error) => error instanceof api.ApiError && error.status === 404,
          () => Effect.succeed(null),
        ),
      );
      let history: ReadonlyArray<HistoryMessage> | undefined;
      if (exported === null) {
        const pages: Array<ReadonlyArray<HistoryMessage>> = [];
        let before: number | undefined;
        for (;;) {
          const page = yield* api.getHistory(sourceTarget, sessionId, {
            limit: RESUME_PAGE_SIZE,
            before,
            agent: fromAgentValue,
          });
          pages.unshift(page.messages);
          if (page.start <= 0 || page.messages.length === 0) break;
          before = page.start;
        }
        history = pages.flat();
      }
      const summary = yield* api.importSession(target(node, token), {
        agent: to,
        ...(exported === null ? { history } : { session: exported }),
        cwd: Option.getOrUndefined(cwd),
        title: Option.getOrUndefined(title),
        model: Option.getOrUndefined(model),
      });
      if (json) return yield* printJson(summary);
      yield* printSession(summary);
    }),
).pipe(
  Command.withDescription(
    '"Resume on…" — pull a session from the source node and write it into an agent store here',
  ),
);

const sessionsRestoreCommand = Command.make(
  "restore",
  {
    sessionId: sessionIdArg,
    confirm: Options.boolean("confirm").pipe(
      Options.withDefault(false),
      Options.withDescription("Required — restore writes real files under the session's cwd"),
    ),
    path: Options.text("path").pipe(
      Options.optional,
      Options.withDescription("File to revert through the session's recorded diffs"),
    ),
    toolCallId: Options.text("tool-call-id").pipe(
      Options.optional,
      Options.withDescription("Revert only this tool call's change to --path"),
    ),
    checkpoint: Options.text("checkpoint").pipe(
      Options.optional,
      Options.withDescription("Materialize a recorded snapshot ref (see `sessions checkpoints`)"),
    ),
    paths: Options.text("paths").pipe(
      Options.repeated,
      Options.withDescription("Narrow a --checkpoint restore to these files — repeatable"),
    ),
    agent: agentQueryOption,
    node: nodeOption,
    token: tokenOption,
    json: jsonOption,
  },
  ({ sessionId, confirm, path, toolCallId, checkpoint, paths, agent, node, token, json }) =>
    Effect.gen(function* () {
      if (!confirm) {
        return yield* Effect.fail(
          new Error("Pass --confirm — restore writes real files under the session's cwd"),
        );
      }
      const pathValue = Option.getOrUndefined(path);
      const checkpointValue = Option.getOrUndefined(checkpoint);
      if ((pathValue === undefined) === (checkpointValue === undefined)) {
        return yield* Effect.fail(new Error("Pass exactly one of --path or --checkpoint"));
      }
      const selector =
        pathValue !== undefined
          ? { path: pathValue, toolCallId: Option.getOrUndefined(toolCallId) }
          : {
              checkpoint: checkpointValue as string,
              paths: paths.length === 0 ? undefined : paths,
            };
      const result = yield* api.restoreSession(
        target(node, token),
        sessionId,
        selector,
        Option.getOrUndefined(agent),
      );
      if (json) return yield* printJson(result);
      for (const file of result.restored) {
        yield* Console.log(`${file.action}\t${file.path}`);
      }
      for (const file of result.skipped) {
        yield* Console.log(`skipped\t${file.path}\t${file.reason}`);
      }
    }),
).pipe(
  Command.withDescription(
    "POST /api/sessions/:id/restore — revert workspace files via recorded diffs or a checkpoint ref",
  ),
);

const sessionsRewindCommand = Command.make(
  "rewind",
  {
    sessionId: sessionIdArg,
    confirm: Options.boolean("confirm").pipe(
      Options.withDefault(false),
      Options.withDescription("Required — rewind deletes stored history"),
    ),
    nodeId: Options.integer("node-id").pipe(
      Options.optional,
      Options.withDescription("Keep this history node and everything before it"),
    ),
    turns: Options.integer("turns").pipe(
      Options.optional,
      Options.withDescription("Drop the last N user turns"),
    ),
    checkpoint: Options.text("checkpoint").pipe(
      Options.optional,
      Options.withDescription("Rewind to a recorded snapshot ref"),
    ),
    agent: agentQueryOption,
    node: nodeOption,
    token: tokenOption,
    json: jsonOption,
  },
  ({ sessionId, confirm, nodeId, turns, checkpoint, agent, node, token, json }) =>
    Effect.gen(function* () {
      if (!confirm) {
        return yield* Effect.fail(
          new Error("Pass --confirm — rewind truncates the session's stored transcript"),
        );
      }
      const selectors = [
        Option.isSome(nodeId) ? { nodeId: nodeId.value } : undefined,
        Option.isSome(turns) ? { turns: turns.value } : undefined,
        Option.isSome(checkpoint) ? { checkpoint: checkpoint.value } : undefined,
      ].filter((selector) => selector !== undefined);
      if (selectors.length !== 1) {
        return yield* Effect.fail(
          new Error("Pass exactly one of --node-id, --turns, --checkpoint"),
        );
      }
      const result = yield* api.rewindSession(
        target(node, token),
        sessionId,
        selectors[0] as { nodeId: number } | { turns: number } | { checkpoint: string },
        Option.getOrUndefined(agent),
      );
      if (json) return yield* printJson(result);
      yield* Console.log(
        `Rewound session ${sessionId}: kept ${result.kept}, removed ${result.removed}`,
      );
    }),
).pipe(
  Command.withDescription(
    "POST /api/sessions/:id/rewind — truncate the transcript at a node, turn count or checkpoint",
  ),
);

const sessionsGroup = Command.make("sessions").pipe(
  Command.withSubcommands([
    sessionsListCommand,
    sessionsCreateCommand,
    sessionsAttachCommand,
    promptCommand,
    sessionsCancelCommand,
    sessionsPermissionCommand,
    sessionsHistoryCommand,
    sessionsCheckpointsCommand,
    sessionsExportCommand,
    sessionsStreamCommand,
    sessionsRunCommand,
    sessionsMetaCommand,
    sessionsRenameCommand,
    sessionsDeleteCommand,
    sessionsConvertCommand,
    sessionsImportCommand,
    sessionsResumeCommand,
    sessionsRestoreCommand,
    sessionsRewindCommand,
  ]),
  Command.withDescription("Node ops against /api/sessions — run against a live sepia node"),
);

// --- projects ------------------------------------------------------------------

/** `projects pull`/`push` stream SSE frames — an `error` frame fails the command. */
const runTransfer = (
  effect: (onFrame: (frame: api.SseFrame) => void) => Effect.Effect<void, api.ApiError>,
): Effect.Effect<void, Error | api.ApiError> =>
  Effect.gen(function* () {
    let failed: string | undefined;
    yield* effect((frame) => {
      printFrame(frame);
      if (frame.event === "error") {
        try {
          const data = JSON.parse(frame.data) as { error?: unknown };
          failed = typeof data.error === "string" ? data.error : "transfer failed";
        } catch {
          failed = "transfer failed";
        }
      }
    });
    if (failed !== undefined) {
      return yield* Effect.fail(new Error(failed));
    }
  });

const projectsGroup = Command.make("projects").pipe(
  Command.withSubcommands([
    Command.make(
      "list",
      { node: nodeOption, token: tokenOption, json: jsonOption },
      ({ node, token, json }) =>
        Effect.gen(function* () {
          const projects = yield* api.listProjects(target(node, token));
          if (json) return yield* printJson({ projects });
          for (const project of projects) yield* Console.log(`${project.id}\t${project.name}`);
        }),
    ).pipe(Command.withDescription("GET /api/projects")),
    Command.make(
      "create",
      {
        name: Args.text({ name: "name" }),
        node: nodeOption,
        token: tokenOption,
      },
      ({ name, node, token }) =>
        Effect.gen(function* () {
          const project = yield* api.createProject(target(node, token), name);
          yield* Console.log(`${project.id}\t${project.name}`);
        }),
    ).pipe(Command.withDescription("POST /api/projects — create a project")),
    Command.make(
      "init",
      {
        name: Args.text({ name: "name" }),
        node: nodeOption,
        token: tokenOption,
      },
      ({ name, node, token }) =>
        Effect.gen(function* () {
          const project = yield* api.createProject(target(node, token), name);
          yield* Console.log(`${project.id}\t${project.name}`);
        }),
    ).pipe(
      Command.withDescription("Alias of `projects create` — init a project (repo) on the node"),
    ),
    Command.make(
      "export",
      {
        projectId: Args.text({ name: "project-id" }).pipe(
          Args.withDescription("Project id on the node"),
        ),
        out: Options.text("out").pipe(
          Options.optional,
          Options.withDescription("Write the NDJSON bundle to this file (default: stdout)"),
        ),
        node: nodeOption,
        token: tokenOption,
      },
      ({ projectId, out, node, token }) =>
        Effect.gen(function* () {
          const bundle = yield* api.exportProject(target(node, token), projectId);
          const outPath = Option.getOrUndefined(out);
          if (outPath === undefined || outPath === "-") {
            return yield* Console.log(bundle.trimEnd());
          }
          const fs = yield* Fs.FileSystem;
          yield* fs.writeFileString(outPath, bundle);
          yield* Console.log(`Exported project ${projectId} to ${outPath}`);
        }),
    ).pipe(
      Command.withDescription(
        "GET /api/projects/:id/export — the project's NDJSON bundle (sessions as full IR)",
      ),
    ),
    Command.make(
      "import",
      {
        file: Args.file({ name: "file" }).pipe(
          Args.withDescription("An NDJSON bundle from `projects export`"),
        ),
        node: nodeOption,
        token: tokenOption,
        json: jsonOption,
      },
      ({ file, node, token, json }) =>
        Effect.gen(function* () {
          const fs = yield* Fs.FileSystem;
          const bundle = yield* fs
            .readFileString(file)
            .pipe(Effect.mapError((cause) => new Error(`Cannot read ${file}: ${String(cause)}`)));
          const summary = yield* api.importProject(target(node, token), bundle);
          if (json) return yield* printJson(summary);
          yield* Console.log(
            `Imported "${summary.project.name}" (${summary.project.id}): ` +
              `${summary.imported.length} session(s), ${summary.skipped.length} skipped` +
              (summary.truncated ? " — bundle was truncated" : ""),
          );
          for (const row of summary.imported) {
            yield* Console.log(`  ${row.agent}:${row.id}\t${row.title}`);
          }
        }),
    ).pipe(
      Command.withDescription(
        "POST /api/projects/import — write a bundle into the node's stores (idempotent by id)",
      ),
    ),
    Command.make(
      "pull",
      {
        projectId: Args.text({ name: "project-id" }).pipe(
          Args.withDescription("Project id on the source node"),
        ),
        from: Options.text("from").pipe(
          Options.withDescription("Source node URL (e.g. http://thinkpad:8787)"),
        ),
        sourceToken: Options.text("source-token").pipe(
          Options.optional,
          Options.withDescription("Bearer token for the source node"),
        ),
        node: nodeOption,
        token: tokenOption,
      },
      ({ projectId, from, sourceToken, node, token }) =>
        runTransfer((onFrame) =>
          api.pullProject(
            target(node, token),
            {
              source: {
                url: from,
                ...(Option.isSome(sourceToken) ? { token: sourceToken.value } : {}),
              },
              project: projectId,
            },
            onFrame,
          ),
        ),
    ).pipe(
      Command.withDescription(
        "POST /api/projects/pull — this node fetches the project bundle from --from and imports it",
      ),
    ),
    Command.make(
      "push",
      {
        projectId: Args.text({ name: "project-id" }).pipe(
          Args.withDescription("Project id on this node"),
        ),
        to: Options.text("to").pipe(
          Options.withDescription("Target node URL (e.g. http://thinkpad:8787)"),
        ),
        targetToken: Options.text("target-token").pipe(
          Options.optional,
          Options.withDescription("Bearer token for the target node"),
        ),
        node: nodeOption,
        token: tokenOption,
      },
      ({ projectId, to, targetToken, node, token }) =>
        runTransfer((onFrame) =>
          api.pushProject(
            target(node, token),
            projectId,
            {
              target: {
                url: to,
                ...(Option.isSome(targetToken) ? { token: targetToken.value } : {}),
              },
            },
            onFrame,
          ),
        ),
    ).pipe(
      Command.withDescription(
        "POST /api/projects/:id/push — this node POSTs the bundle to --to's /api/projects/import",
      ),
    ),
    Command.make(
      "rename",
      {
        projectId: Args.text({ name: "project-id" }),
        name: Args.text({ name: "name" }),
        node: nodeOption,
        token: tokenOption,
      },
      ({ projectId, name, node, token }) =>
        api
          .renameProject(target(node, token), projectId, name)
          .pipe(Effect.andThen(Console.log(`Renamed project ${projectId}`))),
    ).pipe(Command.withDescription("PATCH /api/projects/:id")),
    Command.make(
      "delete",
      {
        projectId: Args.text({ name: "project-id" }),
        node: nodeOption,
        token: tokenOption,
      },
      ({ projectId, node, token }) =>
        api
          .deleteProject(target(node, token), projectId)
          .pipe(Effect.andThen(Console.log(`Deleted project ${projectId}`))),
    ).pipe(Command.withDescription("DELETE /api/projects/:id")),
  ]),
  Command.withDescription("Node-local project grouping (/api/projects)"),
);

// --- config --------------------------------------------------------------------

const configGroup = Command.make("config").pipe(
  Command.withSubcommands([
    Command.make(
      "get",
      { node: nodeOption, token: tokenOption, json: jsonOption },
      ({ node, token, json }) =>
        Effect.gen(function* () {
          const config = yield* api.getConfig(target(node, token));
          if (json) return yield* printJson(config);
          for (const [key, value] of Object.entries(config)) {
            yield* Console.log(`${key}\t${JSON.stringify(value)}`);
          }
        }),
    ).pipe(Command.withDescription("GET /api/config — the node's server-side UI state")),
    Command.make(
      "set",
      {
        key: Args.text({ name: "key" }),
        value: Args.text({ name: "value" }).pipe(
          Args.withDescription("JSON when parseable, else a plain string"),
        ),
        node: nodeOption,
        token: tokenOption,
      },
      ({ key, value, node, token }) =>
        Effect.gen(function* () {
          let parsed: unknown = value;
          try {
            parsed = JSON.parse(value);
          } catch {
            // Not JSON — stored as a plain string.
          }
          yield* api.setConfig(target(node, token), key, parsed);
          yield* Console.log(`${key} = ${JSON.stringify(parsed)}`);
        }),
    ).pipe(Command.withDescription("PATCH /api/config/:key")),
  ]),
  Command.withDescription("The node's server-side config (/api/config)"),
);

// --- servers -------------------------------------------------------------------

const serverAuthOptions = {
  authToken: Options.text("auth-token").pipe(
    Options.optional,
    Options.withDescription("Bearer token the node uses to call the managed server"),
  ),
  authPassword: Options.text("auth-password").pipe(
    Options.optional,
    Options.withDescription("Basic-auth password for the managed server"),
  ),
  authUser: Options.text("auth-user").pipe(
    Options.optional,
    Options.withDescription("Basic-auth user for the managed server (default: sepia)"),
  ),
  noAuth: Options.boolean("no-auth").pipe(
    Options.withDefault(false),
    Options.withDescription("Store no credential — clear auth on update"),
  ),
};

const serverSshOptions = {
  sshUser: Options.text("ssh-user").pipe(
    Options.optional,
    Options.withDescription("SSH login for the tunnel to the managed server"),
  ),
  sshHost: Options.text("ssh-host").pipe(
    Options.optional,
    Options.withDescription("SSH host for the tunnel to the managed server"),
  ),
  sshPort: Options.integer("ssh-port").pipe(
    Options.withDefault(22),
    Options.withDescription("SSH port for the tunnel"),
  ),
  sshKey: Options.text("ssh-key").pipe(
    Options.optional,
    Options.withDescription("Path to a private key, or an inline PEM"),
  ),
  noSsh: Options.boolean("no-ssh").pipe(
    Options.withDefault(false),
    Options.withDescription("No SSH tunnel — clear ssh on update"),
  ),
};

interface ServerFlagValues {
  readonly label: string;
  readonly host: string;
  readonly port: number;
  readonly scheme: "http" | "https";
  readonly authToken: Option.Option<string>;
  readonly authPassword: Option.Option<string>;
  readonly authUser: Option.Option<string>;
  readonly noAuth: boolean;
  readonly sshUser: Option.Option<string>;
  readonly sshHost: Option.Option<string>;
  readonly sshPort: number;
  readonly sshKey: Option.Option<string>;
  readonly noSsh: boolean;
}

const buildServerInput = (flags: ServerFlagValues): Effect.Effect<api.ServerInput, Error> =>
  Effect.gen(function* () {
    if (Option.isSome(flags.authToken) && Option.isSome(flags.authPassword)) {
      return yield* Effect.fail(new Error("--auth-token and --auth-password conflict"));
    }
    if (Option.isSome(flags.authToken) && flags.noAuth) {
      return yield* Effect.fail(new Error("--auth-token and --no-auth conflict"));
    }
    if (Option.isSome(flags.authPassword) && flags.noAuth) {
      return yield* Effect.fail(new Error("--auth-password and --no-auth conflict"));
    }
    const sshUser = Option.getOrUndefined(flags.sshUser);
    const sshHost = Option.getOrUndefined(flags.sshHost);
    if ((sshUser === undefined) !== (sshHost === undefined)) {
      return yield* Effect.fail(new Error("--ssh-user and --ssh-host must be passed together"));
    }
    if (sshUser !== undefined && flags.noSsh) {
      return yield* Effect.fail(new Error("--ssh-user/--ssh-host and --no-ssh conflict"));
    }
    const auth: api.ServerInput["auth"] = flags.noAuth
      ? null
      : Option.isSome(flags.authToken)
        ? { type: "token", secret: flags.authToken.value }
        : Option.isSome(flags.authPassword)
          ? {
              type: "password",
              secret: flags.authPassword.value,
              ...(Option.isSome(flags.authUser) ? { user: flags.authUser.value } : {}),
            }
          : null;
    const ssh: api.ServerInput["ssh"] =
      flags.noSsh || sshUser === undefined || sshHost === undefined
        ? null
        : {
            user: sshUser,
            host: sshHost,
            port: flags.sshPort,
            ...(Option.isSome(flags.sshKey) ? { key: flags.sshKey.value } : {}),
          };
    return {
      label: flags.label,
      host: flags.host,
      port: flags.port,
      scheme: flags.scheme,
      auth,
      ssh,
    };
  });

const serverSharedOptions = {
  label: Options.text("label").pipe(Options.withDescription("Display name for the managed server")),
  host: Options.text("host").pipe(Options.withDescription("Hostname or IP of the managed server")),
  port: Options.integer("port").pipe(
    Options.withDefault(8787),
    Options.withDescription("Port the managed server's API listens on"),
  ),
  scheme: Options.choice("scheme", ["http", "https"]).pipe(
    Options.withDefault("http" as const),
    Options.withDescription("Upstream protocol"),
  ),
  ...serverAuthOptions,
  ...serverSshOptions,
};

const serversGroup = Command.make("servers").pipe(
  Command.withSubcommands([
    Command.make(
      "list",
      { node: nodeOption, token: tokenOption, json: jsonOption },
      ({ node, token, json }) =>
        Effect.gen(function* () {
          const data = yield* api.listServers(target(node, token));
          if (json) return yield* printJson(data);
          for (const server of data.servers) {
            yield* Console.log(
              `${server.id}\t${server.label}\t${server.scheme}://${server.host}:${server.port}`,
            );
          }
          if (data.error !== undefined) yield* Console.error(`warning: ${data.error}`);
        }),
    ).pipe(Command.withDescription("GET /api/servers — the managed-server registry")),
    Command.make("add", { ...serverSharedOptions, node: nodeOption, token: tokenOption }, (flags) =>
      Effect.gen(function* () {
        const input = yield* buildServerInput(flags);
        const server = yield* api.createServer(target(flags.node, flags.token), input);
        yield* Console.log(
          `${server.id}\t${server.label}\t${server.scheme}://${server.host}:${server.port}`,
        );
      }),
    ).pipe(Command.withDescription("POST /api/servers — register a managed server")),
    Command.make(
      "update",
      {
        serverId: Args.text({ name: "server-id" }),
        ...serverSharedOptions,
        node: nodeOption,
        token: tokenOption,
      },
      (flags) =>
        Effect.gen(function* () {
          const input = yield* buildServerInput(flags);
          yield* api.updateServer(target(flags.node, flags.token), flags.serverId, input);
          yield* Console.log(`Updated server ${flags.serverId}`);
        }),
    ).pipe(
      Command.withDescription(
        "PATCH /api/servers/:id — replace a registry entry (all fields required)",
      ),
    ),
    Command.make(
      "remove",
      {
        serverId: Args.text({ name: "server-id" }),
        node: nodeOption,
        token: tokenOption,
      },
      ({ serverId, node, token }) =>
        api
          .removeServer(target(node, token), serverId)
          .pipe(Effect.andThen(Console.log(`Removed server ${serverId}`))),
    ).pipe(Command.withDescription("DELETE /api/servers/:id")),
    Command.make(
      "tunnel-up",
      {
        serverId: Args.text({ name: "server-id" }),
        node: nodeOption,
        token: tokenOption,
      },
      ({ serverId, node, token }) =>
        Effect.gen(function* () {
          const { localPort } = yield* api.tunnelUp(target(node, token), serverId);
          yield* Console.log(`Tunnel up: 127.0.0.1:${localPort} → ${serverId}`);
        }),
    ).pipe(Command.withDescription("POST /api/servers/:id/tunnel — ensure the SSH forward is up")),
    Command.make(
      "tunnel-down",
      {
        serverId: Args.text({ name: "server-id" }),
        node: nodeOption,
        token: tokenOption,
      },
      ({ serverId, node, token }) =>
        api
          .tunnelDown(target(node, token), serverId)
          .pipe(Effect.andThen(Console.log(`Tunnel down for ${serverId}`))),
    ).pipe(Command.withDescription("DELETE /api/servers/:id/tunnel — drop the SSH forward")),
  ]),
  Command.withDescription(
    "The managed-server registry (/api/servers) — gateway peers the node proxies to",
  ),
);

// --- push ----------------------------------------------------------------------

const pushGroup = Command.make("push").pipe(
  Command.withSubcommands([
    Command.make("vapid", { node: nodeOption, token: tokenOption }, ({ node, token }) =>
      Effect.gen(function* () {
        const { publicKey } = yield* api.getVapid(target(node, token));
        yield* Console.log(publicKey);
      }),
    ).pipe(Command.withDescription("GET /api/push/vapid — the node's web-push public key")),
    Command.make(
      "subscribe",
      {
        subscription: Args.text({ name: "subscription" }).pipe(
          Args.withDescription("A push subscription JSON ({endpoint, keys:{auth,p256dh}, prefs?})"),
        ),
        node: nodeOption,
        token: tokenOption,
      },
      ({ subscription, node, token }) =>
        Effect.gen(function* () {
          let parsed: unknown;
          try {
            parsed = JSON.parse(subscription);
          } catch {
            return yield* Effect.fail(new Error("subscription must be valid JSON"));
          }
          yield* api.pushSubscribe(target(node, token), parsed);
          yield* Console.log("Subscribed");
        }),
    ).pipe(Command.withDescription("POST /api/push/subscribe")),
    Command.make(
      "unsubscribe",
      {
        endpoint: Args.text({ name: "endpoint" }),
        node: nodeOption,
        token: tokenOption,
      },
      ({ endpoint, node, token }) =>
        api
          .pushUnsubscribe(target(node, token), endpoint)
          .pipe(Effect.andThen(Console.log("Unsubscribed"))),
    ).pipe(Command.withDescription("DELETE /api/push/subscribe")),
  ]),
  Command.withDescription("Web-push subscription management (/api/push/*)"),
);

/** Every command group that talks to a node — spread into the root subcommand list. */
export const nodeCommands = [
  sessionsGroup,
  projectsGroup,
  configGroup,
  serversGroup,
  pushGroup,
  nodeCommand,
  healthCommand,
  agentsCommand,
  userCommand,
  fsCommand,
  eventsCommand,
  redeemCommand,
] as const;

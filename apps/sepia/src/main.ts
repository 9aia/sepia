import { Args, Command, Options } from "@effect/cli";
import { BunContext, BunRuntime } from "@effect/platform-bun";
import * as Fs from "@effect/platform/FileSystem";
import * as Path from "@effect/platform/Path";
import { Console, Effect, Layer, Option } from "effect";
import { homedir } from "node:os";
import {
  ClaudeCode,
  ClaudeCodeRepository,
  Cline,
  ClineRepository,
  ClineStore,
  Conversion,
  ConversionError,
  CursorRepository,
  Session,
  SessionRepository,
  SqliteStorage,
  openSessionsDb,
  type SessionRepositoryService,
} from "sepia-core";
import { parseEnv, SEPIA_VERSION, startServer, type ServerEnv } from "sepia-server/serve";
import { configGroup } from "./config-commands";
import { nodeCommands, promptCommand } from "./node-commands";
import { PAIR_CODE_TTL_MS, writePairCodeFile } from "./pair";

const defaultDbPath = `${homedir()}/.local/share/devin/cli/sessions.db`;
const defaultDataDir = `${homedir()}/.cline/data`;
const defaultClaudeDir = `${homedir()}/.claude`;
const defaultCursorDir = `${homedir()}/.cursor`;
const defaultSepiaHome = process.env.SEPIA_HOME ?? `${homedir()}/.local/share/sepia`;

/** The four session stores the verbs read and write. */
const STORES = ["devin", "cline", "claude", "cursor"] as const;
type StoreId = (typeof STORES)[number];

const dbOption = Options.file("db").pipe(
  Options.withDefault(defaultDbPath),
  Options.withDescription("Path to the Devin sessions SQLite database"),
);

// The `--*-dir` flags double as store selectors: passing one names that
// store unless `--from`/`--to` says otherwise; when the store is selected
// another way, a missing flag falls back to the agent's own default dir.
const dataDirOption = Options.directory("data-dir").pipe(
  Options.optional,
  Options.withDescription("Path to the Cline CLI data directory"),
);

const claudeDirOption = Options.directory("claude-dir").pipe(
  Options.optional,
  Options.withDescription("Path to the Claude Code data directory (its `projects/` tree is used)"),
);

const cursorDirOption = Options.directory("cursor-dir").pipe(
  Options.optional,
  Options.withDescription("Path to the Cursor data directory"),
);

const fromOption = Options.choice("from", STORES).pipe(
  Options.optional,
  Options.withDescription("Store to read the session from (devin, cline, claude, cursor)"),
);

const toOption = Options.choice("to", STORES).pipe(
  Options.optional,
  Options.withDescription("Store to write the session into (devin, cline, claude, cursor)"),
);

/** The directories a command line resolved, ready for store layers. */
interface StoreDirs {
  readonly db: string;
  readonly dataDir?: string;
  readonly claudeDir?: string;
  readonly cursorDir?: string;
}

const dataDirOf = (dirs: StoreDirs): string => dirs.dataDir ?? defaultDataDir;
const claudeDirOf = (dirs: StoreDirs): string => dirs.claudeDir ?? defaultClaudeDir;
const cursorDirOf = (dirs: StoreDirs): string => dirs.cursorDir ?? defaultCursorDir;

/**
 * The store an explicit `--*-dir` names — the flag would be noise
 * otherwise. `claimed` excludes the store the sibling role (`--to` vs
 * `--from`) already resolved to, so `install --to claude --claude-dir X`
 * doesn't turn the source into claude too.
 */
const dirStore = (dirs: StoreDirs, claimed?: StoreId): StoreId | undefined =>
  dirs.claudeDir !== undefined && claimed !== "claude"
    ? "claude"
    : dirs.cursorDir !== undefined && claimed !== "cursor"
      ? "cursor"
      : dirs.dataDir !== undefined && claimed !== "cline"
        ? "cline"
        : undefined;

/**
 * Resolve which store a role (`--from`/`--to`) names. An explicit flag
 * wins; otherwise an explicit `--*-dir` names its store; last resort is
 * the verb's own default.
 */
const resolveStore = (
  flag: Option.Option<StoreId>,
  dirs: StoreDirs,
  fallback: StoreId,
  claimed?: StoreId,
): StoreId => Option.getOrElse(flag, () => dirStore(dirs, claimed) ?? fallback);

/** The repository service for a non-devin store — devin goes through SqliteStorage. */
const repoService = (store: StoreId, dirs: StoreDirs): SessionRepositoryService => {
  switch (store) {
    case "cline":
      return ClineRepository.makeClineSessionRepository({ dataDir: dataDirOf(dirs) });
    case "claude":
      return ClaudeCodeRepository.makeClaudeCodeSessionRepository({
        projectsDir: `${claudeDirOf(dirs)}/projects`,
      });
    case "cursor":
      return CursorRepository.makeCursorSessionRepository({ cursorDir: cursorDirOf(dirs) });
    case "devin":
      throw new Error("devin is served by SqliteStorage, not repoService");
  }
};

/** A `SessionRepository` layer for `store`; devin opens read-only unless asked to write. */
const repoLayer = (store: StoreId, dirs: StoreDirs, options?: { readonly write?: boolean }) =>
  store === "devin"
    ? options?.write === true
      ? SqliteStorage.layer(dirs.db)
      : SqliteStorage.layerReadonly(dirs.db)
    : Layer.succeed(SessionRepository, repoService(store, dirs));

/** `session` re-keyed for `--id` — make() needs plain props, not a class spread. */
const renameSession = (session: Session, id: string | undefined): Session =>
  id === undefined || id === session.id
    ? session
    : Session.make({
        id,
        title: session.title,
        workingDirectory: session.workingDirectory,
        backendType: session.backendType,
        agentMode: session.agentMode,
        model: session.model,
        createdAt: session.createdAt,
        lastActivityAt: session.lastActivityAt,
        mainChainId: session.mainChainId,
        shellLastSeenIndex: session.shellLastSeenIndex,
        cogsJson: session.cogsJson,
        workspaceDirs: session.workspaceDirs,
        hidden: session.hidden,
        parentSessionId: session.parentSessionId,
        agentId: session.agentId,
        checkpoints: session.checkpoints,
        metadata: session.metadata,
        nodes: session.nodes,
        promptHistory: session.promptHistory,
      });

/**
 * Write `session` into `store`. Devin goes through `importSession` (cogs
 * grafting, existing-session skip); cline through `ClineStore.install`
 * (live-owner guard, index row, generated id when `--id` is absent);
 * claude/cursor through `repo.save` — an existing id refuses unless
 * `--force`, since a save rewrites the store in place.
 */
const installInto = (
  store: StoreId,
  dirs: StoreDirs,
  session: Session,
  options: {
    readonly id?: string;
    readonly force?: boolean;
    /** Log line `importSession` prints on a devin write. */
    readonly importedLog?: string;
  },
) => {
  // `--id` renames for the stores that key the write on session.id.
  const renamed = renameSession(session, options.id);
  switch (store) {
    case "devin":
      return Conversion.importSession(renamed, options.importedLog).pipe(
        Effect.provide(SqliteStorage.layer(dirs.db)),
      );
    case "cline":
      return Effect.gen(function* () {
        const clineStore = yield* ClineStore.ClineStore;
        const installed = yield* clineStore.install(session, options.id, {
          force: options.force,
        });
        yield* Console.log(`Installed session ${installed} into ${dataDirOf(dirs)}`);
        yield* Console.log(`Resume it with: cline --id ${installed} -m <model>`);
        return installed;
      }).pipe(Effect.provide(ClineStore.layer(openSessionsDb, dataDirOf(dirs))));
    case "claude":
    case "cursor": {
      const repo = repoService(store, dirs);
      return Effect.gen(function* () {
        const exists = yield* repo.hasSession(renamed.id);
        if (exists && options.force !== true) {
          return yield* Effect.fail(
            new ConversionError({
              message: `Session ${renamed.id} already exists in the ${store} store; pass --force to overwrite`,
              cause: null,
            }),
          );
        }
        yield* repo.save(renamed);
        yield* Console.log(`Installed session ${renamed.id} into the ${store} store`);
        return renamed.id;
      });
    }
  }
};

/** Fetch a session from `store`, failing with the shared not-found error. */
const getSession = (store: StoreId, dirs: StoreDirs, sessionId: string) =>
  Effect.gen(function* () {
    const repo = yield* SessionRepository;
    const opt = yield* repo.getById(sessionId);
    if (Option.isNone(opt)) {
      return yield* Effect.fail(
        new ConversionError({ message: `Session not found: ${sessionId}`, cause: null }),
      );
    }
    return opt.value;
  }).pipe(Effect.provide(repoLayer(store, dirs)));

/** How `readSessionPath` decoded the source — the log lines name it. */
type ImportedKind = "cline" | "claude" | "json";

const KIND_LABEL: Record<ImportedKind, string> = {
  cline: "Cline",
  claude: "Claude",
  json: "JSON",
};

/**
 * Read a session off disk for `import` — a Cline session directory, a
 * Claude `.jsonl` transcript, or a `SessionJson` export file (what
 * `sepia export` and `GET /api/sessions/:id/export` produce).
 */
const readSessionPath = (
  inputPath: string,
  sessionId?: string,
): Effect.Effect<
  { readonly session: Session; readonly kind: ImportedKind },
  ConversionError,
  Fs.FileSystem | Path.Path
> =>
  Effect.gen(function* () {
    const fs = yield* Fs.FileSystem;
    const info = yield* fs
      .stat(inputPath)
      .pipe(
        Effect.mapError(
          () =>
            new ConversionError({ message: `Import source not found: ${inputPath}`, cause: null }),
        ),
      );
    if (info.type === "Directory") {
      const session = yield* Cline.fromDirectory(inputPath, sessionId);
      return { session, kind: "cline" as const };
    }
    if (inputPath.endsWith(".jsonl")) {
      const session = yield* ClaudeCode.fromFile(
        inputPath,
        sessionId === undefined ? undefined : { id: sessionId },
      );
      return { session, kind: "claude" as const };
    }
    const raw = yield* fs
      .readFileString(inputPath)
      .pipe(
        Effect.mapError(
          (cause) => new ConversionError({ message: `Failed to read ${inputPath}`, cause }),
        ),
      );
    const session = yield* Effect.try({
      try: () => Conversion.sessionFromJson(JSON.parse(raw)),
      catch: (cause) =>
        new ConversionError({
          message: `Import source is neither a Cline dir, a Claude .jsonl, nor a session JSON: ${inputPath}`,
          cause,
        }),
    });
    return {
      session: renameSession(session, sessionId),
      kind: "json" as const,
    };
  });

const importCommand = Command.make(
  "import",
  {
    source: Args.text({ name: "path" }).pipe(
      Args.withDescription(
        "Session to import — a Cline session dir, a Claude .jsonl transcript, or a session JSON export",
      ),
    ),
    db: dbOption,
    dataDir: dataDirOption,
    claudeDir: claudeDirOption,
    cursorDir: cursorDirOption,
    to: toOption,
    sessionId: Options.text("session-id").pipe(
      Options.optional,
      Options.withDescription("Override the imported session id"),
    ),
    force: Options.boolean("force").pipe(
      Options.withDefault(false),
      Options.withDescription("Overwrite a session that already exists in the target store"),
    ),
  },
  ({ source, db, dataDir, claudeDir, cursorDir, to, sessionId, force }) => {
    const dirs: StoreDirs = {
      db,
      dataDir: Option.getOrUndefined(dataDir),
      claudeDir: Option.getOrUndefined(claudeDir),
      cursorDir: Option.getOrUndefined(cursorDir),
    };
    const target = resolveStore(to, dirs, "devin");
    return Effect.gen(function* () {
      const { session, kind } = yield* readSessionPath(source, Option.getOrUndefined(sessionId));
      yield* installInto(target, dirs, session, {
        id: Option.getOrUndefined(sessionId),
        force,
        importedLog: `Imported ${KIND_LABEL[kind]} session ${session.id} into storage`,
      });
    });
  },
).pipe(
  Command.withDescription(
    "Import a session (Cline dir, Claude .jsonl, or session JSON) into a store",
  ),
);

const exportCommand = Command.make(
  "export",
  {
    sessionId: Args.text({ name: "session-id" }),
    out: Args.text({ name: "out" }).pipe(
      Args.optional,
      Args.withDescription(
        "Output file (or directory) for the session JSON; stdout when omitted — required as a directory for --format cline",
      ),
    ),
    format: Options.choice("format", ["json", "cline"]).pipe(
      Options.withDefault("json" as const),
      Options.withDescription("Export format: the session IR JSON, or Cline session files"),
    ),
    db: dbOption,
    dataDir: dataDirOption,
    claudeDir: claudeDirOption,
    cursorDir: cursorDirOption,
    from: fromOption,
  },
  ({ sessionId, out, format, db, dataDir, claudeDir, cursorDir, from }) => {
    const dirs: StoreDirs = {
      db,
      dataDir: Option.getOrUndefined(dataDir),
      claudeDir: Option.getOrUndefined(claudeDir),
      cursorDir: Option.getOrUndefined(cursorDir),
    };
    const source = resolveStore(from, dirs, "devin");
    const outPath = Option.getOrUndefined(out);
    if (format === "cline") {
      if (outPath === undefined) {
        return Effect.fail(
          new ConversionError({
            message: "export --format cline needs an out directory",
            cause: null,
          }),
        );
      }
      return Conversion.exportCline(sessionId, outPath).pipe(
        Effect.provide(repoLayer(source, dirs)),
      );
    }
    return Effect.gen(function* () {
      const session = yield* getSession(source, dirs, sessionId);
      const json = JSON.stringify(Conversion.sessionToJson(session), null, 2);
      if (outPath === undefined || outPath === "-") {
        yield* Console.log(json);
        return;
      }
      const fs = yield* Fs.FileSystem;
      const info = yield* fs.stat(outPath).pipe(Effect.option);
      const filePath =
        Option.isSome(info) && info.value.type === "Directory"
          ? `${outPath}/${session.id}.session.json`
          : outPath;
      yield* fs.writeFileString(filePath, `${json}\n`);
      yield* Console.log(`Exported session ${sessionId} to ${filePath}`);
    });
  },
).pipe(
  Command.withDescription(
    "Export a session from a store — the session IR JSON (default) or Cline session files",
  ),
);

const installCommand = Command.make(
  "install",
  {
    sessionId: Args.text({ name: "session-id" }),
    db: dbOption,
    dataDir: dataDirOption,
    claudeDir: claudeDirOption,
    cursorDir: cursorDirOption,
    from: fromOption,
    to: toOption,
    id: Options.text("id").pipe(
      Options.optional,
      Options.withDescription("Session id to install under (defaults to a generated one)"),
    ),
    force: Options.boolean("force").pipe(
      Options.withDefault(false),
      Options.withDescription("Replace a session that still belongs to a live owner"),
    ),
  },
  ({ sessionId, db, dataDir, claudeDir, cursorDir, from, to, id, force }) => {
    const dirs: StoreDirs = {
      db,
      dataDir: Option.getOrUndefined(dataDir),
      claudeDir: Option.getOrUndefined(claudeDir),
      cursorDir: Option.getOrUndefined(cursorDir),
    };
    const target = resolveStore(to, dirs, "cline");
    const source = resolveStore(from, dirs, "devin", target);
    return Effect.gen(function* () {
      const session = yield* getSession(source, dirs, sessionId);
      const requested = Option.getOrUndefined(id);
      yield* installInto(target, dirs, session, {
        id: requested,
        force,
        importedLog: `Installed session ${requested ?? session.id} into the devin store`,
      });
    });
  },
).pipe(
  Command.withDescription(
    "Copy a session into a store ready to resume — cline (default), claude, cursor or devin",
  ),
);

const deleteCommand = Command.make(
  "delete",
  {
    sessionId: Args.text({ name: "session-id" }),
    db: dbOption,
    dataDir: dataDirOption,
    claudeDir: claudeDirOption,
    cursorDir: cursorDirOption,
    from: fromOption,
  },
  ({ sessionId, db, dataDir, claudeDir, cursorDir, from }) => {
    const dirs: StoreDirs = {
      db,
      dataDir: Option.getOrUndefined(dataDir),
      claudeDir: Option.getOrUndefined(claudeDir),
      cursorDir: Option.getOrUndefined(cursorDir),
    };
    const source = resolveStore(from, dirs, "devin");
    return Effect.gen(function* () {
      const repo = yield* SessionRepository;
      yield* repo.delete(sessionId);
      yield* Console.log(`Deleted session ${sessionId} from the ${source} store`);
    }).pipe(Effect.provide(repoLayer(source, dirs, { write: true })));
  },
).pipe(Command.withDescription("Delete a session from a store"));

const listCommand = Command.make(
  "list",
  {
    db: dbOption,
    dataDir: dataDirOption,
    claudeDir: claudeDirOption,
    cursorDir: cursorDirOption,
    from: fromOption,
  },
  ({ db, dataDir, claudeDir, cursorDir, from }) => {
    const dirs: StoreDirs = {
      db,
      dataDir: Option.getOrUndefined(dataDir),
      claudeDir: Option.getOrUndefined(claudeDir),
      cursorDir: Option.getOrUndefined(cursorDir),
    };
    const source = resolveStore(from, dirs, "devin");
    return Conversion.listSessions().pipe(Effect.provide(repoLayer(source, dirs)));
  },
).pipe(Command.withDescription("List sessions in a store"));

const pairCommand = Command.make(
  "pair",
  {
    home: Options.text("home").pipe(
      Options.withDefault(defaultSepiaHome),
      Options.withDescription("SEPIA_HOME of the node to pair with"),
    ),
    url: Options.text("url").pipe(
      Options.withDefault(`http://localhost:${process.env.PORT ?? "8787"}`),
      Options.withDescription("The node's URL, printed for the pairing UI"),
    ),
  },
  ({ home, url }) =>
    Effect.sync(() => {
      // Mint = write the code file the running server consumes; whoever can
      // write $SEPIA_HOME is the machine owner, which is the whole gate.
      const { code } = writePairCodeFile(home);
      console.log(`Pairing code (valid ${Math.round(PAIR_CODE_TTL_MS / 1000)}s, single use):`);
      console.log(`\n  ${code}\n`);
      console.log(`Node URL: ${url}`);
      console.log(`Enter both in Settings → Nodes → "Pair with code" before it expires.`);
    }),
).pipe(
  Command.withDescription("Print a one-time pairing code — the UI exchanges it via POST /api/pair"),
);

// Boots the node: the /api/* control plane plus the embedded web UI on one
// port — "one binary per machine, any machine hosts the UI". `--no-ui` (or
// SEPIA_UI=off) makes it an API-only node; every SEPIA_* env var applies.
const serveCommand = Command.make(
  "serve",
  {
    noUi: Options.boolean("no-ui").pipe(
      Options.withDefault(false),
      Options.withDescription("Serve only the API — do not serve the bundled web UI"),
    ),
  },
  ({ noUi }) =>
    Effect.promise(() => {
      const parsed = parseEnv();
      const env: ServerEnv = {
        ...parsed,
        ui: { ...parsed.ui, enabled: parsed.ui.enabled && !noUi },
      };
      return startServer(env).then(() => undefined);
    }).pipe(Effect.andThen(Effect.never)),
).pipe(
  Command.withDescription(
    "Serve the sepia node — API plus the embedded web UI on one port (SEPIA_* env configures it)",
  ),
);

// `sepia version` prints the release stamp (MAJOR.YYMMDD.HHMM — see
// tools/version.ts); the same value /api/node reports.
const versionCommand = Command.make("version", {}, () => Console.log(SEPIA_VERSION)).pipe(
  Command.withDescription("Print the sepia version stamp"),
);

// The local-store verbs under an explicit group — same command objects as
// the top-level aliases, kept both for backwards compatibility (`sepia
// list …`) and discoverability (`sepia store list`).
const storeGroup = Command.make("store").pipe(
  Command.withSubcommands([
    listCommand,
    exportCommand,
    importCommand,
    installCommand,
    deleteCommand,
  ]),
  Command.withDescription(
    "Store ops — read and write local agent stores directly (no running node needed)",
  ),
);

const sepia = Command.make("sepia").pipe(
  Command.withSubcommands([
    // Node ops — hit a running node's REST API (--node/--token,
    // SEPIA_NODE_URL/SEPIA_TOKEN).
    ...nodeCommands,
    promptCommand,
    // Config ops — agent config IR over skills/rules/commands/hooks.
    configGroup,
    // Store ops — the original local-store verbs, kept top-level.
    storeGroup,
    listCommand,
    exportCommand,
    importCommand,
    installCommand,
    deleteCommand,
    // Node lifecycle + pairing.
    pairCommand,
    serveCommand,
    versionCommand,
  ]),
  Command.withDescription(
    "Drive a sepia node over its API, or convert sessions between the Devin, Cline, Claude and Cursor stores",
  ),
);

const cli = Command.run(sepia, {
  name: "sepia",
  version: SEPIA_VERSION,
});

cli(process.argv).pipe(Effect.provide(BunContext.layer), BunRuntime.runMain);

import { Args, Command, Options } from "@effect/cli";
import { BunContext, BunRuntime } from "@effect/platform-bun";
import { Effect, Option } from "effect";
import { homedir } from "node:os";
import { ClineStore, Conversion, SqliteStorage, openSessionsDb } from "sepia-core";

const defaultDbPath = `${homedir()}/.local/share/devin/cli/sessions.db`;
const defaultDataDir = `${homedir()}/.cline/data`;

const dbOption = Options.file("db").pipe(
  Options.withDefault(defaultDbPath),
  Options.withDescription("Path to the Devin sessions SQLite database"),
);

const dataDirOption = Options.directory("data-dir").pipe(
  Options.withDefault(defaultDataDir),
  Options.withDescription("Path to the Cline CLI data directory"),
);

const importCommand = Command.make(
  "import",
  {
    clineDir: Args.directory({ name: "cline-dir" }),
    db: dbOption,
    sessionId: Options.text("session-id").pipe(
      Options.optional,
      Options.withDescription("Override the imported session id"),
    ),
  },
  ({ clineDir, db, sessionId }) =>
    Conversion.importCline(clineDir, Option.getOrUndefined(sessionId)).pipe(
      Effect.provide(SqliteStorage.layer(db)),
    ),
).pipe(Command.withDescription("Import a Cline session directory into the Devin store"));

const exportCommand = Command.make(
  "export",
  {
    sessionId: Args.text({ name: "session-id" }),
    outDir: Args.directory({ name: "out-dir" }),
    db: dbOption,
  },
  ({ sessionId, outDir, db }) =>
    Conversion.exportCline(sessionId, outDir).pipe(Effect.provide(SqliteStorage.layerReadonly(db))),
).pipe(Command.withDescription("Export a Devin session to a directory as Cline session files"));

const installCommand = Command.make(
  "install",
  {
    sessionId: Args.text({ name: "session-id" }),
    db: dbOption,
    dataDir: dataDirOption,
    id: Options.text("id").pipe(
      Options.optional,
      Options.withDescription("Session id to install under (defaults to a generated one)"),
    ),
    force: Options.boolean("force").pipe(
      Options.withDefault(false),
      Options.withDescription("Replace a session that still belongs to a live owner"),
    ),
  },
  ({ sessionId, db, dataDir, id, force }) =>
    Conversion.installCline(sessionId, dataDir, Option.getOrUndefined(id), { force }).pipe(
      Effect.provide(SqliteStorage.layerReadonly(db)),
      Effect.provide(ClineStore.layer(openSessionsDb, dataDir)),
    ),
).pipe(
  Command.withDescription(
    "Export a Devin session into the Cline CLI store, ready for `cline --id <session-id>`",
  ),
);

const listCommand = Command.make("list", { db: dbOption }, ({ db }) =>
  Conversion.listSessions().pipe(Effect.provide(SqliteStorage.layerReadonly(db))),
).pipe(Command.withDescription("List sessions in the Devin store"));

const sepia = Command.make("sepia").pipe(
  Command.withSubcommands([importCommand, exportCommand, installCommand, listCommand]),
  Command.withDescription("Convert sessions between the Devin and Cline stores"),
);

const cli = Command.run(sepia, {
  name: "sepia",
  version: "0.0.1",
});

cli(process.argv).pipe(Effect.provide(BunContext.layer), BunRuntime.runMain);

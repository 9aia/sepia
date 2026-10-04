import { Args, Command, Options } from "@effect/cli";
import { BunContext, BunRuntime } from "@effect/platform-bun";
import { Effect, Option } from "effect";
import { homedir } from "node:os";
import { ClineStore, Conversion, SqliteStorage, openSessionsDb } from "sepia-core";
import { PAIR_CODE_TTL_MS, writePairCodeFile } from "./pair";

const defaultDbPath = `${homedir()}/.local/share/devin/cli/sessions.db`;
const defaultDataDir = `${homedir()}/.cline/data`;
const defaultSepiaHome = process.env.SEPIA_HOME ?? `${homedir()}/.local/share/sepia`;

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

const sepia = Command.make("sepia").pipe(
  Command.withSubcommands([importCommand, exportCommand, installCommand, listCommand, pairCommand]),
  Command.withDescription("Convert sessions between the Devin and Cline stores"),
);

const cli = Command.run(sepia, {
  name: "sepia",
  version: "0.0.1",
});

cli(process.argv).pipe(Effect.provide(BunContext.layer), BunRuntime.runMain);

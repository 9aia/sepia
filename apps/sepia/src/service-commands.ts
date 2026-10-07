import { Command, Options } from "@effect/cli";
import { Console, Effect } from "effect";
import { existsSync, realpathSync } from "node:fs";
import { homedir } from "node:os";
import { detectBackend, defaultEnvFile, type ServiceSpec } from "./service/backend";

/**
 * The `sepia service` verb group — install the node as an OS service so it
 * survives reboots and session idles (systemd user units on Linux, launchd
 * plists on macOS). `install` writes the unit plus a `~/.config/sepia/env`
 * template (created once, never overwritten — secrets stay out of the
 * unit), then enables + starts it. Every verb degrades to a clear error on
 * unsupported platforms or a missing control tool.
 */

const systemOption = Options.boolean("system").pipe(
  Options.withDefault(false),
  Options.withDescription("Install/manage the system-level unit instead of the per-user one"),
);

// Exec is only meaningful at install time — status/logs/uninstall must not
// resolve it (a dev checkout refuses, and these verbs don't need it anyway).
const spec = (system: boolean, envFile?: string): ServiceSpec => ({
  exec: [],
  envFile: envFile ?? defaultEnvFile(homedir()),
  system,
});

/**
 * The command the unit runs — the compiled binary when `sepia` IS the
 * binary (process.execPath points at it). Refuses when running under the
 * bun interpreter (a dev checkout) — a unit pinned to a source tree rots.
 */
const resolveExec = (): ReadonlyArray<string> => {
  const exec = realpathSync(process.execPath);
  if (exec.endsWith("/bun") || exec.endsWith("\\bun.exe")) {
    throw new Error(
      `sepia is running under the bun interpreter (${exec}) — build the compiled ` +
        `binary first (vp run build:binary) or pass --exec "bun <abs path>/apps/sepia/src/main.ts serve"`,
    );
  }
  return [exec, "serve"];
};

const backend = Effect.promise(async () => {
  const found = await detectBackend();
  if (found === null) {
    throw new Error(`sepia service is not supported on ${process.platform} yet`);
  }
  return found;
});

const fail = (verb: string) => (error: unknown) => {
  const message = error instanceof Error ? error.message : String(error);
  return Console.error(`sepia service ${verb}: ${message}`).pipe(
    Effect.andThen(
      Effect.sync(() => {
        process.exit(1);
      }),
    ),
  );
};

const installCommand = Command.make(
  "install",
  {
    system: systemOption,
    linger: Options.boolean("linger").pipe(
      Options.withDefault(false),
      Options.withDescription("Start the user unit at boot without login (loginctl enable-linger)"),
    ),
    exec: Options.text("exec").pipe(
      Options.optional,
      Options.withDescription("Override the ExecStart command (default: this binary + ' serve')"),
    ),
    envFile: Options.text("env-file").pipe(
      Options.optional,
      Options.withDescription("Environment file path (default: ~/.config/sepia/env)"),
    ),
  },
  ({ system, linger, exec, envFile }) =>
    Effect.gen(function* () {
      const b = yield* backend;
      const s: ServiceSpec = {
        exec: exec._tag === "Some" ? exec.value.split(/\s+/).filter(Boolean) : resolveExec(),
        envFile: envFile._tag === "Some" ? envFile.value : defaultEnvFile(homedir()),
        system,
      };
      yield* Effect.promise(() => b.install(s));
      yield* Console.log(`installed ${b.id} unit at ${b.unitPath(s)} — service is running`);
      yield* Console.log(`env: edit ${s.envFile} then 'sepia service restart'`);
      if (linger) {
        const proc = Bun.spawnSync(["loginctl", "enable-linger"], {
          stdio: ["ignore", "pipe", "pipe"],
        });
        if (proc.exitCode === 0) yield* Console.log("linger enabled — starts at boot");
        else yield* Console.log(`linger needs permission — run: sudo loginctl enable-linger $USER`);
      }
    }).pipe(Effect.catchAll(fail("install"))),
).pipe(Command.withDescription("Install the sepia node as an OS service (systemd/launchd)"));

const uninstallCommand = Command.make(
  "uninstall",
  {
    system: systemOption,
    purge: Options.boolean("purge").pipe(
      Options.withDefault(false),
      Options.withDescription("Also delete the env file (kept by default)"),
    ),
  },
  ({ system, purge }) =>
    Effect.gen(function* () {
      const b = yield* backend;
      const s = spec(system);
      yield* Effect.promise(() => b.uninstall(s, purge));
      yield* Console.log(`removed ${b.id} unit at ${b.unitPath(s)}`);
      if (!purge && existsSync(s.envFile))
        yield* Console.log(`env file kept at ${s.envFile} (--purge removes it)`);
    }).pipe(Effect.catchAll(fail("uninstall"))),
).pipe(Command.withDescription("Stop, disable and remove the sepia service"));

const statusCommand = Command.make("status", { system: systemOption }, ({ system }) =>
  Effect.gen(function* () {
    const b = yield* backend;
    const st = yield* Effect.promise(() => b.status(spec(system)));
    yield* Console.log(
      `installed=${st.installed} enabled=${st.enabled} active=${st.active}` +
        (st.pid === undefined ? "" : ` pid=${st.pid}`),
    );
    if (st.detail !== "") yield* Console.log(st.detail);
  }).pipe(Effect.catchAll(fail("status"))),
).pipe(Command.withDescription("Show whether the sepia service is installed, enabled and running"));

const restartCommand = Command.make("restart", { system: systemOption }, ({ system }) =>
  Effect.gen(function* () {
    const b = yield* backend;
    yield* Effect.promise(() => b.restart(spec(system)));
    yield* Console.log("restarted");
  }).pipe(Effect.catchAll(fail("restart"))),
).pipe(Command.withDescription("Restart the sepia service"));

const logsCommand = Command.make(
  "logs",
  {
    system: systemOption,
    follow: Options.boolean("f").pipe(
      Options.withDefault(false),
      Options.withDescription("Follow the log stream (journalctl -f / tail -f)"),
    ),
  },
  ({ system, follow }) =>
    Effect.gen(function* () {
      const b = yield* backend;
      const code = yield* Effect.promise(() => b.logs(spec(system), follow));
      if (code !== 0) return yield* Effect.fail(new Error(`logs exited ${code}`));
    }).pipe(Effect.catchAll(fail("logs"))),
).pipe(Command.withDescription("Print (or follow, -f) the sepia service log"));

export const serviceGroup = Command.make("service").pipe(
  Command.withSubcommands([
    installCommand,
    uninstallCommand,
    statusCommand,
    restartCommand,
    logsCommand,
  ]),
  Command.withDescription(
    "Manage the sepia node as an OS service (systemd user unit / launchd plist)",
  ),
);

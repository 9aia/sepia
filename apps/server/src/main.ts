import { homedir } from "node:os";
import { Effect, Layer, ManagedRuntime } from "effect";
import { ClineRepository, SessionRepository, SqliteStorage } from "sepia-core";
import { builtinAgents, spawnAgent } from "sepia-acp";
import { ControlPlane, layer as controlPlaneLayer, mergeRepositories } from "sepia-session-control";
import { createApp } from "./app";
import { parseEnv } from "./env";
import { createMetaStore } from "./meta";
import { otelLayer } from "./telemetry";

const isLoopback = (value: string): boolean =>
  value === "localhost" || value === "::1" || value === "[::1]" || value.startsWith("127.");

const env = (() => {
  try {
    return parseEnv();
  } catch (error) {
    console.error(`sepia-server: ${error instanceof Error ? error.message : String(error)}`);
    process.exit(1);
  }
})();

if (!isLoopback(env.host) && (env.token === undefined || env.token === "")) {
  console.error(
    `sepia-server refuses to bind ${env.host} without SEPIA_TOKEN. Set SEPIA_TOKEN or bind a loopback address (SEPIA_HOST=127.0.0.1).`,
  );
  process.exit(1);
}

// `SEPIA_AGENT_DEVIN_COMMAND="bun /path/agent.mjs"` overrides the spawn argv, so
// operators can point at wrappers or custom agents without rebuilding.
const agentCommand = (id: string, fallback: ReadonlyArray<string>): ReadonlyArray<string> => {
  const override = process.env[`SEPIA_AGENT_${id.toUpperCase()}_COMMAND`];
  return override !== undefined && override.trim() !== "" ? override.trim().split(/\s+/) : fallback;
};

const agents = builtinAgents.map((spec) => {
  const effective = { ...spec, command: agentCommand(spec.id, spec.command) };
  return {
    id: spec.id,
    label: spec.label,
    spawn: (options: {
      readonly cwd: string;
      readonly model?: string;
      readonly fallbacks?: ReadonlyArray<string>;
    }) => spawnAgent(effective, options),
  };
});

// Overlay Cline's on-disk sessions onto the Devin store so the UI lists both.
// The overlay is read-only and degrades to empty when the dir is missing.
const clineDir = process.env.SEPIA_CLINE_DIR ?? `${homedir()}/.cline/data`;

const repoLayer = Layer.unwrapEffect(
  Effect.gen(function* () {
    const devin = yield* SessionRepository;
    const cline = ClineRepository.makeClineSessionRepository({ dataDir: clineDir });
    return Layer.succeed(SessionRepository, mergeRepositories(devin, [cline]));
  }).pipe(Effect.provide(SqliteStorage.layerReadonly(env.dbPath))),
);

const appLayer = controlPlaneLayer({
  agents,
  defaultAgentId: "devin",
  probeCwd: process.cwd(),
}).pipe(Layer.provide(repoLayer));

const layer = env.otel.enabled
  ? Layer.mergeAll(
      appLayer,
      otelLayer({ endpoint: env.otel.endpoint, serviceName: env.otel.serviceName }),
    )
  : appLayer;

const runtime = ManagedRuntime.make(layer);

const plane = await runtime.runPromise(ControlPlane).catch((error: unknown) => {
  console.error(`sepia-server failed to start: ${String(error)}`);
  process.exit(1);
});

const server = Bun.serve({
  hostname: env.host,
  port: env.port,
  fetch: createApp(plane, {
    token: env.token,
    allowedOrigins: env.origins,
    run: (effect) => runtime.runPromise(effect),
    meta: createMetaStore(env.metaPath),
    convert: { dbPath: env.dbPath, clineDir },
  }),
});
console.log(`sepia-server listening on ${server.url.href}`);

let shuttingDown = false;
const shutdown = (signal: NodeJS.Signals): void => {
  if (shuttingDown) return;
  shuttingDown = true;
  console.log(`sepia-server received ${signal}, shutting down`);
  void runtime
    .runPromise(plane.closeAll())
    .catch(() => undefined)
    .finally(() => process.exit(0));
};

for (const signal of ["SIGINT", "SIGTERM", "SIGHUP"] as const) {
  process.on(signal, () => shutdown(signal));
}

// A stray rejection or throw should stop the server cleanly (closing agent
// subprocesses) instead of dying mid-request and orphaning children.
process.on("unhandledRejection", (reason) => {
  console.error("sepia-server unhandled rejection:", reason);
  shutdown("SIGTERM");
});
process.on("uncaughtException", (error) => {
  console.error("sepia-server uncaught exception:", error);
  shutdown("SIGTERM");
});

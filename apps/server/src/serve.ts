import { homedir } from "node:os";
import { Effect, Layer, ManagedRuntime } from "effect";
import {
  ClaudeCodeRepository,
  ClineRepository,
  CursorRepository,
  SessionRepository,
  SqliteStorage,
} from "sepia-core";
import { builtinAgents, spawnAgent } from "sepia-acp";
import { ControlPlane, layer as controlPlaneLayer, mergeRepositories } from "sepia-session-control";
import { createApp } from "./app";
import { parseEnv, type ServerEnv } from "./env";
import { makeRewinders } from "./rewinders";
import { createMetaStore } from "./meta";
import { loadNodeIdentity } from "./node";
import { createPairing } from "./pair";
import { createServerStore } from "./servers";
import { createTunnelManager } from "./ssh";
import { otelLayer } from "./telemetry";
import { createUiAssets } from "./ui";

export { parseEnv, type ServerEnv };

const isLoopback = (value: string): boolean =>
  value === "localhost" || value === "::1" || value === "[::1]" || value.startsWith("127.");

/**
 * Boots the sepia node — `/api/*` plus (when enabled and a bundle is
 * available) the embedded web UI — and installs signal handlers. Shared by
 * `apps/server/src/main.ts` (`bun src/main.ts`) and `sepia serve`.
 */
export const startServer = async (env: ServerEnv): Promise<ReturnType<typeof Bun.serve>> => {
  if (!isLoopback(env.host) && (env.token === undefined || env.token === "")) {
    throw new Error(
      `sepia-server refuses to bind ${env.host} without SEPIA_TOKEN. Set SEPIA_TOKEN or bind a loopback address (SEPIA_HOST=127.0.0.1).`,
    );
  }

  // `SEPIA_AGENT_DEVIN_COMMAND="bun /path/agent.mjs"` overrides the spawn argv, so
  // operators can point at wrappers or custom agents without rebuilding.
  const agentCommand = (id: string, fallback: ReadonlyArray<string>): ReadonlyArray<string> => {
    const override = process.env[`SEPIA_AGENT_${id.toUpperCase()}_COMMAND`];
    return override !== undefined && override.trim() !== ""
      ? override.trim().split(/\s+/)
      : fallback;
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

  // Overlay Cline's, Claude Code's and Cursor's on-disk sessions onto the
  // Devin store so the UI lists all four. All overlays are read-only and
  // degrade to empty when the dir is missing.
  const clineDir = process.env.SEPIA_CLINE_DIR ?? `${homedir()}/.cline/data`;
  const claudeDir = process.env.SEPIA_CLAUDE_DIR ?? `${homedir()}/.claude`;
  const cursorDir = process.env.SEPIA_CURSOR_DIR ?? `${homedir()}/.cursor`;

  const repoLayer = Layer.unwrapEffect(
    Effect.gen(function* () {
      const devin = yield* SessionRepository;
      const cline = ClineRepository.makeClineSessionRepository({ dataDir: clineDir });
      const claude = ClaudeCodeRepository.makeClaudeCodeSessionRepository({
        projectsDir: `${claudeDir}/projects`,
      });
      const cursor = CursorRepository.makeCursorSessionRepository({ cursorDir });
      return Layer.succeed(SessionRepository, mergeRepositories(devin, [cline, claude, cursor]));
    }).pipe(Effect.provide(SqliteStorage.layerReadonly(env.dbPath))),
  );

  const appLayer = controlPlaneLayer({
    agents,
    defaultAgentId: "devin",
    probeCwd: process.cwd(),
    // Claude file-history checkpoints restore from `<claudeDir>/file-history`.
    fileHistoryDir: `${claudeDir}/file-history`,
    // Conversation rewind — per-store truncation writers (see rewinders.ts).
    rewinders: makeRewinders({ dbPath: env.dbPath, clineDir, claudeDir, cursorDir }),
  }).pipe(Layer.provide(repoLayer));

  const layer = env.otel.enabled
    ? Layer.mergeAll(
        appLayer,
        otelLayer({ endpoint: env.otel.endpoint, serviceName: env.otel.serviceName }),
      )
    : appLayer;

  const runtime = ManagedRuntime.make(layer);

  const plane = await runtime.runPromise(ControlPlane).catch((error: unknown) => {
    throw new Error(`sepia-server failed to start: ${String(error)}`);
  });

  const tunnels = createTunnelManager({ keyDir: `${env.home}/ssh-keys` });

  const ui = env.ui.enabled ? createUiAssets({ dir: env.ui.dir }) : undefined;

  const server = Bun.serve({
    hostname: env.host,
    port: env.port,
    fetch: createApp(plane, {
      token: env.token,
      allowedOrigins: env.origins,
      run: (effect) => runtime.runPromise(effect),
      meta: createMetaStore(env.metaPath),
      convert: { dbPath: env.dbPath, clineDir },
      node: loadNodeIdentity(env.nodePath, env.nodeName),
      // `sepia pair` writes $SEPIA_HOME/pair-code; issued credentials persist
      // (as sha256 hashes) in $SEPIA_HOME/tokens.json.
      pairing: createPairing({
        codeFile: `${env.home}/pair-code`,
        tokensFile: `${env.home}/tokens.json`,
      }),
      servers: createServerStore(env.serversPath, env.serversKeyPath),
      tunnels,
      ui,
    }),
  });
  console.log(
    `sepia-server listening on ${server.url.href}${ui === undefined ? " (API only — no UI bundle)" : ""}`,
  );

  let shuttingDown = false;
  const shutdown = (signal: NodeJS.Signals): void => {
    if (shuttingDown) return;
    shuttingDown = true;
    console.log(`sepia-server received ${signal}, shutting down`);
    tunnels.closeAll();
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

  return server;
};

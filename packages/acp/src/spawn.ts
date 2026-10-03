import { spawn, type ChildProcess } from "node:child_process";
import { Readable, Writable } from "node:stream";
import * as acp from "@agentclientprotocol/sdk";
import { createAcpConnection } from "./AcpConnection.js";
import { StderrTail } from "./stderr.js";
import type { AcpConnection, AgentSpec, SpawnOptions } from "./types.js";

/** Environment variables a spawned agent needs; everything else is dropped. */
const ALLOWED_ENV_KEYS = [
  "PATH",
  "HOME",
  "USER",
  "SHELL",
  "TMPDIR",
  "LANG",
  "LC_ALL",
  "TERM",
  "WINDSURF_API_KEY",
] as const;

/**
 * Builds the child environment from an allowlist; `spec.env`/`options.env` win.
 * `SEPIA_INHERIT_ENV=1` opts back into forwarding the whole parent environment.
 */
export const buildChildEnv = (
  spec: AgentSpec,
  options: SpawnOptions,
  base: NodeJS.ProcessEnv = process.env,
): Record<string, string> => {
  const env: Record<string, string> = {};
  if (base.SEPIA_INHERIT_ENV === "1") {
    for (const [key, value] of Object.entries(base)) {
      if (value !== undefined) env[key] = value;
    }
  } else {
    for (const key of ALLOWED_ENV_KEYS) {
      const value = base[key];
      if (value !== undefined) env[key] = value;
    }
  }
  Object.assign(env, spec.env, options.env);
  return env;
};

export const spawnAgent = async (
  spec: AgentSpec,
  options: SpawnOptions,
): Promise<AcpConnection> => {
  const [command, ...args] = spec.command;
  if (command === undefined) {
    throw new Error(`Agent "${spec.id}" has no command to spawn`);
  }

  const child: ChildProcess = spawn(command, args, {
    cwd: options.cwd,
    env: buildChildEnv(spec, options),
    stdio: ["pipe", "pipe", "pipe"],
  });

  const stderr = new StderrTail({ id: spec.id, debug: process.env.SEPIA_DEBUG === "1" });
  child.stderr?.on("data", (chunk: Buffer) => stderr.push(chunk.toString()));
  child.on("error", (error) => stderr.push(`failed to start: ${String(error)}`));

  if (child.stdin === null || child.stdout === null) {
    throw new Error(`Agent "${spec.id}" did not expose stdio pipes`);
  }

  const stream = acp.ndJsonStream(Writable.toWeb(child.stdin), Readable.toWeb(child.stdout));
  const connection = createAcpConnection(stream, child, { stderr });
  await connection.initialize();
  return connection;
};

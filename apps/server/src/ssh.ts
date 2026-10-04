import { spawn, type ChildProcess } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import { createServer, Socket } from "node:net";
import { join } from "node:path";
import { isInlineKey, type ServerEntry, type ServerSsh } from "./servers";

/**
 * SSH local port-forwarding for managed servers. Rather than linking an ssh2
 * library we shell out to the system `ssh` — it gets the user's own config,
 * agent, and known_hosts for free, and there's nothing new to audit for key
 * handling. `BatchMode` guarantees the child can never block on a prompt.
 */

export interface TunnelArgsOptions {
  /** Override for the `-i` key file (used when the stored key is inline PEM). */
  readonly keyFile?: string;
}

/**
 * argv for `ssh` that forwards 127.0.0.1:localPort → targetHost:targetPort via
 * `ssh.user@ssh.host`. Pure — kept separate from spawning so it stays testable.
 */
export const sshTunnelArgs = (
  ssh: ServerSsh,
  targetHost: string,
  targetPort: number,
  localPort: number,
  options: TunnelArgsOptions = {},
): string[] => {
  const args = [
    "-N", // no remote command
    "-T", // no pty — this is a pipe, not a terminal
    "-o",
    "BatchMode=yes", // never prompt; fail instead of hanging the request
    "-o",
    "ExitOnForwardFailure=yes", // bind failure kills ssh instead of idling
    "-o",
    "StrictHostKeyChecking=accept-new", // TOFU: accept unknown, still pin known
    "-o",
    "ServerAliveInterval=15",
    "-o",
    "ServerAliveCountMax=2",
    "-p",
    String(ssh.port),
    "-L",
    `127.0.0.1:${localPort}:${targetHost}:${targetPort}`,
  ];
  const keyFile =
    options.keyFile ?? (ssh.key !== undefined && !isInlineKey(ssh.key) ? ssh.key : undefined);
  if (keyFile !== undefined) args.push("-i", keyFile);
  args.push(`${ssh.user}@${ssh.host}`);
  return args;
};

/** Reserve an ephemeral loopback port. Small TOCTOU window — ssh retries us. */
const freePort = (): Promise<number> =>
  new Promise((resolve, reject) => {
    const server = createServer();
    server.unref();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      if (address === null || typeof address === "string") {
        server.close(() => reject(new Error("no local port available")));
        return;
      }
      const { port } = address;
      server.close(() => resolve(port));
    });
  });

const TCP_PROBE_MS = 250;

/** The forward is usable once something accepts TCP on the local end. */
const waitForPort = (localPort: number, proc: ChildProcess, timeoutMs: number): Promise<void> =>
  new Promise((resolve, reject) => {
    const deadline = Date.now() + timeoutMs;
    const onExit = (code: number | null) => {
      reject(new Error(`ssh exited before the tunnel came up (code ${code ?? "?"})`));
    };
    proc.once("exit", onExit);
    const tryConnect = (): void => {
      if (Date.now() > deadline) {
        proc.removeListener("exit", onExit);
        reject(new Error("ssh tunnel did not come up in time"));
        return;
      }
      const socket = new Socket();
      socket.setTimeout(TCP_PROBE_MS);
      socket.once("connect", () => {
        socket.destroy();
        proc.removeListener("exit", onExit);
        resolve();
      });
      socket.once("error", () => {
        socket.destroy();
        setTimeout(tryConnect, TCP_PROBE_MS);
      });
      socket.once("timeout", () => {
        socket.destroy();
        setTimeout(tryConnect, TCP_PROBE_MS);
      });
      socket.connect(localPort, "127.0.0.1");
    };
    tryConnect();
  });

const TUNNEL_UP_TIMEOUT_MS = 8_000;

export interface TunnelHandle {
  readonly localPort: number;
}

export interface TunnelManager {
  /** Idempotent — returns the existing forward while its ssh process lives. */
  readonly ensure: (entry: ServerEntry) => Promise<TunnelHandle>;
  readonly localPort: (id: string) => number | undefined;
  readonly close: (id: string) => void;
  readonly closeAll: () => void;
}

interface LiveTunnel {
  readonly proc: ChildProcess;
  readonly localPort: number;
}

/**
 * Spawns and supervises one `ssh -L` per ssh-enabled registry entry. Pasted
 * (inline) keys are materialized to `keyDir/<id>.pem` with mode 0600 for the
 * life of the process — the plaintext key never exists anywhere else on disk
 * besides the encrypted registry file.
 */
export const createTunnelManager = (options: { readonly keyDir: string }): TunnelManager => {
  const live = new Map<string, LiveTunnel>();
  const starting = new Map<string, Promise<TunnelHandle>>();

  const materializeKey = (entry: ServerEntry): string | undefined => {
    const ssh = entry.ssh;
    if (ssh?.key === undefined || !isInlineKey(ssh.key)) return undefined;
    mkdirSync(options.keyDir, { recursive: true, mode: 0o700 });
    const path = join(options.keyDir, `${entry.id}.pem`);
    writeFileSync(path, ssh.key, { mode: 0o600 });
    return path;
  };

  const start = async (entry: ServerEntry): Promise<TunnelHandle> => {
    if (entry.ssh === null) throw new Error("Server has no SSH config");
    const localPort = await freePort();
    const args = sshTunnelArgs(entry.ssh, entry.host, entry.port, localPort, {
      keyFile: materializeKey(entry),
    });
    const proc = spawn("ssh", args, { stdio: ["ignore", "ignore", "pipe"] });
    let stderr = "";
    proc.stderr?.on("data", (chunk: Buffer) => {
      // Cap the buffer — stderr is only surfaced on startup failure.
      if (stderr.length < 4_096) stderr += chunk.toString();
    });
    try {
      await waitForPort(localPort, proc, TUNNEL_UP_TIMEOUT_MS);
    } catch (error) {
      proc.kill("SIGKILL");
      const detail = stderr.trim().split("\n").at(-1);
      throw new Error(
        detail !== undefined && detail !== ""
          ? `SSH tunnel failed: ${detail}`
          : error instanceof Error
            ? error.message
            : "SSH tunnel failed",
      );
    }
    const tunnel: LiveTunnel = { proc, localPort };
    live.set(entry.id, tunnel);
    proc.once("exit", () => {
      if (live.get(entry.id) === tunnel) live.delete(entry.id);
    });
    return { localPort };
  };

  return {
    ensure: (entry) => {
      const existing = live.get(entry.id);
      if (existing !== undefined && existing.proc.exitCode === null) {
        return Promise.resolve({ localPort: existing.localPort });
      }
      const pending = starting.get(entry.id);
      if (pending !== undefined) return pending;
      const promise = start(entry).finally(() => starting.delete(entry.id));
      starting.set(entry.id, promise);
      return promise;
    },
    localPort: (id) => {
      const tunnel = live.get(id);
      return tunnel !== undefined && tunnel.proc.exitCode === null ? tunnel.localPort : undefined;
    },
    close: (id) => {
      const tunnel = live.get(id);
      if (tunnel === undefined) return;
      live.delete(id);
      tunnel.proc.kill("SIGTERM");
      // Reap hard if it ignores SIGTERM (rare with -N, but cheap insurance).
      setTimeout(() => {
        if (tunnel.proc.exitCode === null) tunnel.proc.kill("SIGKILL");
      }, 2_000).unref();
    },
    closeAll: () => {
      for (const [id, tunnel] of live) {
        live.delete(id);
        tunnel.proc.kill("SIGKILL");
      }
    },
  };
};

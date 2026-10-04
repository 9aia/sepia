import { homedir, hostname } from "node:os";

export interface ServerEnv {
  readonly dbPath: string;
  readonly port: number;
  readonly host: string;
  readonly token: string | undefined;
  readonly metaPath: string;
  /** Sepia-owned data dir — node.json, meta.json. */
  readonly home: string;
  /** Stable node identity file (see src/node.ts). */
  readonly nodePath: string;
  /** Display name reported by GET /api/node; defaults to the hostname. */
  readonly nodeName: string;
  /** Encrypted managed-server registry (Settings → Servers). */
  readonly serversPath: string;
  /** 256-bit hex key file encrypting serversPath (SEPIA_SERVERS_KEY overrides). */
  readonly serversKeyPath: string;
  readonly origins: ReadonlyArray<string>;
  readonly otel: {
    readonly enabled: boolean;
    readonly endpoint: string;
    readonly serviceName: string;
  };
}

const DEFAULT_ORIGINS = ["http://localhost:3000", "http://127.0.0.1:3000"];

/** Parses and validates boot-time configuration; throws with a clear message. */
export const parseEnv = (env: NodeJS.ProcessEnv = process.env): ServerEnv => {
  const dbPath = env.SEPIA_DB ?? `${homedir()}/.local/share/devin/cli/sessions.db`;
  if (dbPath.trim() === "") {
    throw new Error("SEPIA_DB must not be empty");
  }

  const rawPort = env.PORT;
  const port = rawPort === undefined || rawPort === "" ? 8787 : Number(rawPort);
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    throw new Error(`PORT must be an integer between 1 and 65535, got "${rawPort ?? ""}"`);
  }

  const configuredOrigins = (env.SEPIA_ORIGINS ?? "")
    .split(",")
    .map((value) => value.trim())
    .filter((value) => value !== "");

  const otelEndpoint =
    env.OTEL_EXPORTER_OTLP_ENDPOINT?.replace(/\/+$/, "") ?? "http://localhost:4318";
  if (env.SEPIA_OTEL !== "0" && !/^https?:\/\//.test(otelEndpoint)) {
    throw new Error(`OTEL_EXPORTER_OTLP_ENDPOINT must be an http(s) URL, got "${otelEndpoint}"`);
  }

  const home = env.SEPIA_HOME ?? `${homedir()}/.local/share/sepia`;

  return {
    dbPath,
    port,
    host: env.SEPIA_HOST ?? "127.0.0.1",
    token: env.SEPIA_TOKEN,
    metaPath: env.SEPIA_META ?? `${home}/meta.json`,
    home,
    nodePath: env.SEPIA_NODE ?? `${home}/node.json`,
    nodeName:
      env.SEPIA_NAME !== undefined && env.SEPIA_NAME.trim() !== "" ? env.SEPIA_NAME : hostname(),
    serversPath: env.SEPIA_SERVERS ?? `${home}/servers.json`,
    serversKeyPath:
      env.SEPIA_SERVERS_KEY_PATH ??
      `${env.XDG_CONFIG_HOME ?? `${homedir()}/.config`}/sepia/servers.key`,
    origins: configuredOrigins.length > 0 ? configuredOrigins : DEFAULT_ORIGINS,
    otel: {
      enabled: env.SEPIA_OTEL !== "0",
      endpoint: otelEndpoint,
      serviceName: env.OTEL_SERVICE_NAME ?? "sepia-server",
    },
  };
};

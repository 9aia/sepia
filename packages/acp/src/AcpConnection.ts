import type { ChildProcess } from "node:child_process";
import * as acp from "@agentclientprotocol/sdk";
import { asArray, asNumberOrNull, asRecord, asString, normalizeUpdate } from "./normalize.js";
import { createPermissionBroker } from "./permissions.js";
import type { StderrSource } from "./stderr.js";
import type {
  AcpCapabilities,
  AcpConnection,
  AcpSessionInfo,
  AcpSessionUpdate,
  PermissionRequest,
} from "./types.js";

/** `AcpConnection` plus the internal `initialize` step run before it is handed out. */
export interface ManagedAcpConnection extends AcpConnection {
  initialize(): Promise<void>;
}

const EXIT_TIMEOUT_MS = 2_000;

const describeExit = (code: number | null, signal: NodeJS.Signals | null): Error =>
  new Error(
    `Agent process exited before responding (code=${code === null ? "null" : code}, signal=${
      signal === null ? "null" : signal
    })`,
  );

const hasExited = (child: ChildProcess): boolean =>
  child.exitCode !== null || child.signalCode !== null;

const waitForExit = (child: ChildProcess, timeoutMs: number): Promise<boolean> =>
  new Promise((resolve) => {
    if (hasExited(child)) {
      resolve(true);
      return;
    }
    const onExit = (): void => {
      clearTimeout(timer);
      resolve(true);
    };
    const timer = setTimeout(() => {
      child.removeListener("exit", onExit);
      resolve(false);
    }, timeoutMs);
    child.once("exit", onExit);
  });

export const mapSessionListResponse = (response: unknown): ReadonlyArray<AcpSessionInfo> =>
  asArray(asRecord(response).sessions).map((entry) => {
    const session = asRecord(entry);
    const meta = asRecord(session._meta);
    return {
      sessionId: asString(session.sessionId) ?? "",
      cwd: asString(session.cwd) ?? "",
      title: asString(session.title) ?? "",
      updatedAt: asString(session.updatedAt) ?? "",
      locked: meta["cognition.ai/isLocked"] === true,
      lockHolderPid: asNumberOrNull(meta["cognition.ai/lockHolderPid"]),
    };
  });

export const createAcpConnection = (
  stream: acp.Stream,
  child: ChildProcess,
  options: { readonly stderr?: StderrSource } = {},
): ManagedAcpConnection => {
  const updateListeners = new Set<(update: AcpSessionUpdate) => void>();
  const permissionListeners = new Set<(request: PermissionRequest) => void>();
  const permissions = createPermissionBroker();
  const pendingRequests = new Set<(error: Error) => void>();
  let exitError: Error | null = null;

  const track = <A>(run: () => Promise<A>): Promise<A> => {
    if (exitError !== null) return Promise.reject(exitError);
    return new Promise<A>((resolve, reject) => {
      const fail = (error: Error): void => {
        pendingRequests.delete(fail);
        reject(error);
      };
      pendingRequests.add(fail);
      run().then(
        (value) => {
          pendingRequests.delete(fail);
          resolve(value);
        },
        (error: unknown) => {
          pendingRequests.delete(fail);
          reject(error instanceof Error ? error : new Error(String(error)));
        },
      );
    });
  };

  child.once("exit", (code, signal) => {
    const error = describeExit(code, signal);
    exitError = error;
    const pending = [...pendingRequests];
    pendingRequests.clear();
    for (const fail of pending) fail(error);
    permissions.failAll(error);
  });

  const conn = acp
    .client({ name: "sepia" })
    .onNotification(acp.methods.client.session.update, ({ params }) => {
      const update = normalizeUpdate(params.update);
      for (const listener of updateListeners) listener(update);
    })
    .onRequest(acp.methods.client.session.requestPermission, ({ params }) => {
      const pending = permissions.begin(params);
      for (const listener of permissionListeners) listener(pending.request);
      return pending.response;
    })
    .connect(stream);

  let capabilities: AcpCapabilities = { loadSession: false, sessionList: false };

  return {
    get capabilities() {
      return capabilities;
    },
    initialize: async () => {
      const result = await track(() =>
        conn.agent.request(acp.methods.agent.initialize, {
          protocolVersion: acp.PROTOCOL_VERSION,
          clientCapabilities: {},
        }),
      );
      capabilities = {
        loadSession: result.agentCapabilities?.loadSession === true,
        sessionList: Boolean(result.agentCapabilities?.sessionCapabilities?.list),
      };
    },
    listSessions: async () =>
      mapSessionListResponse(
        await track(() => conn.agent.request(acp.methods.agent.session.list, {})),
      ),
    newSession: async (cwd) =>
      (
        await track(() =>
          conn.agent.request(acp.methods.agent.session.new, { cwd, mcpServers: [] }),
        )
      ).sessionId,
    loadSession: async (sessionId, cwd) => {
      await track(() =>
        conn.agent.request(acp.methods.agent.session.load, { sessionId, cwd, mcpServers: [] }),
      );
    },
    prompt: async (sessionId, parts) => {
      await track(() =>
        conn.agent.request(acp.methods.agent.session.prompt, {
          sessionId,
          prompt: parts.map((part) => ({ type: "text" as const, text: part.text })),
        }),
      );
    },
    cancel: async (sessionId) => {
      await conn.agent.notify(acp.methods.agent.session.cancel, { sessionId });
    },
    respondToPermission: (requestId, optionId) => permissions.respond(requestId, optionId),
    recentStderr: () => options.stderr?.recent() ?? [],
    onUpdate: (listener) => {
      updateListeners.add(listener);
      return () => {
        updateListeners.delete(listener);
      };
    },
    onPermission: (listener) => {
      permissionListeners.add(listener);
      return () => {
        permissionListeners.delete(listener);
      };
    },
    close: async () => {
      child.stdin?.end();
      if (!hasExited(child)) child.kill();
      if (await waitForExit(child, EXIT_TIMEOUT_MS)) return;
      child.kill("SIGKILL");
      await waitForExit(child, EXIT_TIMEOUT_MS);
    },
  };
};

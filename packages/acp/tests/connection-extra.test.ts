import { EventEmitter } from "node:events";
import type { ChildProcess } from "node:child_process";
import { afterEach, describe, expect, it, vi } from "vite-plus/test";
import * as acp from "@agentclientprotocol/sdk";
import { createAcpConnection, mapSessionListResponse } from "../src/AcpConnection.js";
import type { AcpSessionUpdate, PermissionRequest } from "../src/types.js";

class FakeChild extends EventEmitter {
  readonly stdin = { end: (): void => {} };
  readonly killed: NodeJS.Signals[] = [];
  exitCode: number | null = null;
  signalCode: NodeJS.Signals | null = null;
  exitsOnKill = true;

  kill(signal: NodeJS.Signals = "SIGTERM"): boolean {
    this.killed.push(signal);
    if (this.exitsOnKill) {
      this.exitCode = 0;
      queueMicrotask(() => this.emit("exit", 0, null));
    }
    return true;
  }
}

const asChild = (child: FakeChild): ChildProcess => child as unknown as ChildProcess;

interface Wire {
  readonly stream: acp.Stream;
  readonly written: acp.AnyMessage[];
  readonly enqueue: (message: acp.AnyMessage) => void;
  /** Respond to the most recent request frame with a result payload. */
  readonly respondLast: (result: unknown) => void;
}

const wire = (): Wire => {
  const written: acp.AnyMessage[] = [];
  let enqueue: (message: acp.AnyMessage) => void = () => {};
  const readable = new ReadableStream<acp.AnyMessage>({
    start(controller) {
      enqueue = (message) => controller.enqueue(message);
    },
  });
  const stream: acp.Stream = {
    readable,
    writable: new WritableStream<acp.AnyMessage>({
      write(message) {
        written.push(message);
      },
    }),
  };
  const respondLast = (result: unknown): void => {
    const last = written[written.length - 1] as { id?: number | string };
    enqueue({ jsonrpc: "2.0", id: last.id, result } as acp.AnyMessage);
  };
  return { stream, written, enqueue, respondLast };
};

const tick = (): Promise<void> => new Promise((resolve) => setTimeout(resolve, 20));

afterEach(() => {
  vi.useRealTimers();
});

describe("initialize", () => {
  it("maps agent capabilities from the initialize response", async () => {
    const { stream, written, respondLast } = wire();
    const conn = createAcpConnection(stream, asChild(new FakeChild()));
    expect(conn.capabilities).toEqual({ loadSession: false, sessionList: false });

    const init = conn.initialize();
    await tick();
    const request = written[0] as { method?: string };
    expect(request.method).toBe(acp.methods.agent.initialize);
    respondLast({
      protocolVersion: acp.PROTOCOL_VERSION,
      agentCapabilities: { loadSession: true, sessionCapabilities: { list: {} } },
    });
    await init;
    expect(conn.capabilities).toEqual({ loadSession: true, sessionList: true });
  });

  it("defaults missing capabilities to false", async () => {
    const { stream, respondLast } = wire();
    const conn = createAcpConnection(stream, asChild(new FakeChild()));
    const init = conn.initialize();
    await tick();
    respondLast({ protocolVersion: acp.PROTOCOL_VERSION, agentCapabilities: {} });
    await init;
    expect(conn.capabilities).toEqual({ loadSession: false, sessionList: false });
  });
});

describe("session requests", () => {
  const initialized = async () => {
    const w = wire();
    const conn = createAcpConnection(w.stream, asChild(new FakeChild()));
    const init = conn.initialize();
    await tick();
    w.respondLast({ protocolVersion: acp.PROTOCOL_VERSION, agentCapabilities: {} });
    await init;
    w.written.length = 0;
    return { conn, ...w };
  };

  it("listSessions maps the response including lock metadata", async () => {
    const { conn, respondLast } = await initialized();
    const list = conn.listSessions();
    await tick();
    respondLast({
      sessions: [
        {
          sessionId: "s1",
          cwd: "/w",
          title: "T",
          updatedAt: "2024-01-01T00:00:00Z",
          _meta: { "cognition.ai/isLocked": true, "cognition.ai/lockHolderPid": "42" },
        },
      ],
    });
    await expect(list).resolves.toEqual([
      {
        sessionId: "s1",
        cwd: "/w",
        title: "T",
        updatedAt: "2024-01-01T00:00:00Z",
        locked: true,
        lockHolderPid: 42,
      },
    ]);
  });

  it("newSession returns the created session id", async () => {
    const { conn, written, respondLast } = await initialized();
    const created = conn.newSession("/work/dir");
    await tick();
    const request = written[0] as { method?: string; params?: { cwd?: string } };
    expect(request.method).toBe(acp.methods.agent.session.new);
    expect(request.params?.cwd).toBe("/work/dir");
    respondLast({ sessionId: "fresh-1" });
    await expect(created).resolves.toBe("fresh-1");
  });

  it("loadSession sends the session id and cwd", async () => {
    const { conn, written, respondLast } = await initialized();
    const loaded = conn.loadSession("s1", "/w");
    await tick();
    const request = written[0] as { method?: string; params?: Record<string, unknown> };
    expect(request.method).toBe(acp.methods.agent.session.load);
    expect(request.params).toMatchObject({ sessionId: "s1", cwd: "/w" });
    respondLast({});
    await loaded;
  });

  it("prompt forwards content blocks verbatim", async () => {
    const { conn, written, respondLast } = await initialized();
    const parts = [
      { type: "text", text: "hello" },
      { type: "image", data: "aGk=", mimeType: "image/png", uri: "attachment://hi.png" },
      { type: "audio", data: "AAA=", mimeType: "audio/mpeg" },
      {
        type: "resource",
        resource: { uri: "attachment://notes.md", mimeType: "text/markdown", text: "# hi" },
      },
      {
        type: "resource",
        resource: { uri: "attachment://blob.bin", blob: "AAE=" },
      },
      {
        type: "resource_link",
        uri: "file:///tmp/log.txt",
        name: "log.txt",
        mimeType: "text/plain",
        size: 12,
      },
    ] as const;
    const prompted = conn.prompt("s1", parts);
    await tick();
    const request = written[0] as { method?: string; params?: Record<string, unknown> };
    expect(request.method).toBe(acp.methods.agent.session.prompt);
    expect(request.params).toEqual({ sessionId: "s1", prompt: [...parts] });
    respondLast({ stopReason: "end_turn" });
    await prompted;
  });

  it("cancel writes a notification, not a request", async () => {
    const { conn, written } = await initialized();
    await conn.cancel("s1");
    await tick();
    const note = written[0] as { method?: string; id?: unknown; params?: unknown };
    expect(note.method).toBe(acp.methods.agent.session.cancel);
    expect(note.id).toBeUndefined();
    expect(note.params).toEqual({ sessionId: "s1" });
  });

  it("deleteSession resolves on the agent's response", async () => {
    const { conn, written, respondLast } = await initialized();
    const deleted = conn.deleteSession("s1");
    await tick();
    const request = written[0] as { method?: string; params?: unknown };
    expect(request.method).toBe(acp.methods.agent.session.delete);
    expect(request.params).toEqual({ sessionId: "s1" });
    respondLast({});
    await deleted;
  });

  it("a JSON-RPC error rejects the request", async () => {
    const { conn, written, enqueue } = await initialized();
    const list = conn.listSessions();
    const rejection = expect(list).rejects.toThrow();
    await tick();
    const last = written[written.length - 1] as { id?: number | string };
    enqueue({
      jsonrpc: "2.0",
      id: last.id,
      error: { code: -32603, message: "internal agent failure" },
    } as acp.AnyMessage);
    await rejection;
  });
});

describe("update + permission listeners", () => {
  it("forwards normalized session updates to listeners until unsubscribed", async () => {
    const { stream, enqueue } = wire();
    const conn = createAcpConnection(stream, asChild(new FakeChild()));
    const seen: AcpSessionUpdate[] = [];
    const off = conn.onUpdate((update) => seen.push(update));

    enqueue({
      jsonrpc: "2.0",
      method: acp.methods.client.session.update,
      params: {
        sessionId: "s1",
        update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "hi" } },
      },
    } as acp.AnyMessage);
    await tick();
    expect(seen).toEqual([{ kind: "agent_message_chunk", text: "hi" }]);

    off();
    enqueue({
      jsonrpc: "2.0",
      method: acp.methods.client.session.update,
      params: {
        sessionId: "s1",
        update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "x" } },
      },
    } as acp.AnyMessage);
    await tick();
    expect(seen).toHaveLength(1);
  });

  it("permission requests reach listeners and respondToPermission settles them", async () => {
    const { stream, written, enqueue } = wire();
    const conn = createAcpConnection(stream, asChild(new FakeChild()));
    const requests: PermissionRequest[] = [];
    conn.onPermission((request) => requests.push(request));

    enqueue({
      jsonrpc: "2.0",
      id: 9,
      method: "session/request_permission",
      params: {
        sessionId: "s1",
        toolCall: { toolCallId: "t1", title: "Run ls" },
        options: [{ optionId: "allow", name: "Allow", kind: "allow_once" }],
      },
    } as acp.AnyMessage);
    await tick();

    expect(requests).toHaveLength(1);
    const request = requests[0];
    expect(request?.sessionId).toBe("s1");
    expect(request?.toolCallId).toBe("t1");
    expect(request?.title).toBe("Run ls");
    expect(conn.respondToPermission(request!.requestId, "allow")).toBe(true);
    await tick();
    const response = written.find((m) => (m as { id?: unknown }).id === 9) as {
      result?: { outcome?: { outcome?: string; optionId?: string } };
    };
    expect(response?.result?.outcome).toEqual({ outcome: "selected", optionId: "allow" });
  });

  it("unsubscribing a permission listener stops delivery", async () => {
    const { stream, enqueue } = wire();
    const conn = createAcpConnection(stream, asChild(new FakeChild()));
    const requests: PermissionRequest[] = [];
    const off = conn.onPermission((request) => requests.push(request));
    off();
    enqueue({
      jsonrpc: "2.0",
      id: 9,
      method: "session/request_permission",
      params: { sessionId: "s1", toolCall: {}, options: [] },
    } as acp.AnyMessage);
    await tick();
    expect(requests).toHaveLength(0);
  });
});

describe("close", () => {
  it("escalates to SIGKILL when the child ignores SIGTERM", async () => {
    vi.useFakeTimers();
    const child = new FakeChild();
    child.exitsOnKill = false;
    const conn = createAcpConnection(
      { writable: new WritableStream(), readable: new ReadableStream({ start() {} }) },
      asChild(child),
    );

    const closed = conn.close();
    await vi.advanceTimersByTimeAsync(2_000);
    expect(child.killed).toEqual(["SIGTERM", "SIGKILL"]);

    child.emit("exit", 0, null);
    await closed;
  });
});

describe("mapSessionListResponse", () => {
  it("defaults missing fields and drops lock flags that are absent", () => {
    expect(mapSessionListResponse({})).toEqual([]);
    expect(mapSessionListResponse({ sessions: [{}] })).toEqual([
      { sessionId: "", cwd: "", title: "", updatedAt: "", locked: false, lockHolderPid: null },
    ]);
    expect(mapSessionListResponse("not an object")).toEqual([]);
  });
});

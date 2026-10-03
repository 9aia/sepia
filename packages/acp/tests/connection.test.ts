import { EventEmitter } from "node:events";
import type { ChildProcess } from "node:child_process";
import { expect, test } from "vite-plus/test";
import type * as acp from "@agentclientprotocol/sdk";
import { createAcpConnection } from "../src/AcpConnection.js";
import { StderrTail } from "../src/stderr.js";

class FakeChild extends EventEmitter {
  readonly stdin = { end: (): void => {} };
  readonly killed: NodeJS.Signals[] = [];
  exitCode: number | null = null;
  signalCode: NodeJS.Signals | null = null;

  kill(signal: NodeJS.Signals = "SIGTERM"): boolean {
    this.killed.push(signal);
    this.exitCode = 0;
    queueMicrotask(() => this.emit("exit", 0, null));
    return true;
  }
}

const asChild = (child: FakeChild): ChildProcess => child as unknown as ChildProcess;

const silentStream = (): acp.Stream => ({
  writable: new WritableStream(),
  readable: new ReadableStream({ start() {} }),
});

test("an in-flight request rejects when the child exits", async () => {
  const child = new FakeChild();
  const conn = createAcpConnection(silentStream(), asChild(child));

  const prompt = conn.prompt("s1", [{ type: "text", text: "hi" }]);
  const rejection = expect(prompt).rejects.toThrow(/exited/);

  child.emit("exit", 1, null);
  await rejection;
});

test("requests made after the child exits reject immediately", async () => {
  const child = new FakeChild();
  const conn = createAcpConnection(silentStream(), asChild(child));

  child.emit("exit", 0, null);
  await expect(conn.listSessions()).rejects.toThrow(/exited/);
});

test("respondToPermission returns false for an unknown id", () => {
  const child = new FakeChild();
  const conn = createAcpConnection(silentStream(), asChild(child));
  expect(conn.respondToPermission("nope", "allow")).toBe(false);
});

test("exposes the agent stderr tail", () => {
  const child = new FakeChild();
  const tail = new StderrTail({ id: "devin" });
  tail.push("boom\n");

  const conn = createAcpConnection(silentStream(), asChild(child), { stderr: tail });
  expect(conn.recentStderr()).toEqual(["[agent:devin] boom"]);
});

test("a pending permission request rejects when the child exits", async () => {
  const written: Array<acp.AnyMessage> = [];
  let enqueue: (message: acp.AnyMessage) => void = () => {};
  const readable = new ReadableStream<acp.AnyMessage>({
    start(controller) {
      enqueue = (message) => controller.enqueue(message);
    },
  });
  const stream: acp.Stream = {
    writable: new WritableStream({
      write(message) {
        written.push(message);
      },
    }),
    readable,
  };

  const child = new FakeChild();
  createAcpConnection(stream, asChild(child));

  enqueue({
    jsonrpc: "2.0",
    id: 1,
    method: "session/request_permission",
    params: {
      sessionId: "s1",
      toolCall: { toolCallId: "t1", title: "Run" },
      options: [{ optionId: "allow", name: "Allow", kind: "allow_once" }],
    },
  });
  await new Promise((resolve) => setTimeout(resolve, 20));

  child.emit("exit", 1, null);
  await new Promise((resolve) => setTimeout(resolve, 20));

  const errorResponse = written.find(
    (message) => (message as { readonly error?: unknown }).error !== undefined,
  );
  expect(errorResponse).toBeDefined();
});

test("close kills the child and resolves after it exits", async () => {
  const child = new FakeChild();
  const conn = createAcpConnection(silentStream(), asChild(child));

  await conn.close();
  expect(child.killed).toEqual(["SIGTERM"]);
});

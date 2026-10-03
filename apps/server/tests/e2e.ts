// End-to-end smoke test: real control plane over an in-memory store, real
// Bun.serve, and a fixture ACP agent subprocess. Run with `bun tests/e2e.ts`
// (the vitest suite runs under node and cannot use bun:sqlite / Bun.serve).
import assert from "node:assert/strict";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { Layer, ManagedRuntime } from "effect";
import { spawnAgent } from "sepia-acp";
import { SqliteStorage } from "sepia-core";
import { ControlPlane, layer as controlPlaneLayer } from "sepia-session-control";
import { createApp } from "../src/app";

const fixture = join(dirname(fileURLToPath(import.meta.url)), "fixtures", "fake-agent.mjs");

const agents = [
  {
    id: "devin",
    label: "Fake Devin",
    spawn: ({ cwd }: { readonly cwd: string }) =>
      spawnAgent(
        { id: "devin", label: "Fake Devin", command: [process.execPath, fixture] },
        { cwd },
      ),
  },
];

const layer = controlPlaneLayer({ agents, defaultAgentId: "devin" }).pipe(
  Layer.provide(SqliteStorage.layer(":memory:")),
);
const runtime = ManagedRuntime.make(layer);
const plane = await runtime.runPromise(ControlPlane);

const token = "e2e-token";
const server = Bun.serve({
  hostname: "127.0.0.1",
  port: 0,
  fetch: createApp(plane, { token, logger: () => {} }),
});
const base = server.url.origin;
const auth = { authorization: `Bearer ${token}` };

try {
  const unauthenticated = await fetch(`${base}/api/sessions`);
  assert.equal(unauthenticated.status, 401, "expected 401 without a token");

  const health = await fetch(`${base}/api/health`);
  assert.equal(health.status, 200, "expected health 200 without a token");

  const created = await fetch(`${base}/api/sessions`, {
    method: "POST",
    headers: { ...auth, "content-type": "application/json" },
    body: JSON.stringify({ cwd: process.cwd(), title: "e2e" }),
  });
  assert.equal(created.status, 201, `expected 201, got ${created.status}`);
  const { id } = (await created.json()) as { id: string };
  assert.equal(id, "e2e-session");

  const list = (await (await fetch(`${base}/api/sessions`, { headers: auth })).json()) as {
    sessions: Array<{ id: string }>;
  };
  assert.ok(
    list.sessions.some((session) => session.id === id),
    "created session should be listed",
  );

  const run = await fetch(`${base}/api/agent?sessionId=${encodeURIComponent(id)}`, {
    method: "POST",
    headers: { ...auth, "content-type": "application/json" },
    body: JSON.stringify({
      threadId: "t1",
      runId: "r1",
      messages: [{ id: "m1", role: "user", content: "hi" }],
    }),
  });
  assert.equal(run.status, 200, `expected 200, got ${run.status}`);
  assert.ok(run.body !== null, "expected an SSE body");

  const deadline = Date.now() + 10_000;
  const reader = run.body.getReader();
  const decoder = new TextDecoder();
  let sse = "";
  while (!sse.includes("RUN_FINISHED")) {
    assert.ok(Date.now() < deadline, `timed out waiting for RUN_FINISHED; got:\n${sse}`);
    const { done, value } = await reader.read();
    if (done) break;
    sse += decoder.decode(value, { stream: true });
  }
  await reader.cancel();

  assert.match(sse, /RUN_STARTED/, "missing RUN_STARTED");
  assert.match(sse, /TEXT_MESSAGE_CONTENT/, "missing TEXT_MESSAGE_CONTENT");
  assert.match(sse, /"delta":"ok"|"delta": ?"ok"/, "missing agent text delta");
  assert.match(sse, /RUN_FINISHED/, "missing RUN_FINISHED");

  console.log("e2e: PASS");
} finally {
  server.stop(true);
  await runtime.runPromise(plane.closeAll());
  await runtime.dispose();
}

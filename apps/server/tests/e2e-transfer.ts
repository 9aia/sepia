// End-to-end project transfer: two real Bun.serve nodes over real sqlite
// stores; node B pulls node A's project over HTTP (bundle streamed
// node-to-node, bearer auth on both legs), then pushes a second project
// back. Run with `bun tests/e2e-transfer.ts` — the vitest suite stubs
// bun:sqlite and can't do this.
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { Effect, Layer, ManagedRuntime } from "effect";
import { BunFileSystem } from "@effect/platform-bun";
import { BunPath } from "@effect/platform-bun";
import { MessageNode, Session } from "sepia-core";
import { Conversion } from "sepia-convert";
import { SqliteStorage } from "sepia-devin";
import { ControlPlane, layer as controlPlaneLayer } from "sepia-session-control";
import { createApp } from "../src/app";
import { createMetaStore } from "../src/meta";

const tmp = mkdtempSync(join(tmpdir(), "sepia-e2e-transfer-"));

const fixture = (id: string): Session =>
  Session.make({
    id,
    title: `Session ${id}`,
    workingDirectory: "/work/x",
    model: "test-model",
    createdAt: 1_700_000_000,
    lastActivityAt: 1_700_000_100,
    mainChainId: 1,
    metadata: null,
    nodes: [
      MessageNode.make({
        nodeId: 0,
        role: "user",
        content: "hello",
        createdAt: 1_700_000_000,
        metadata: null,
      }),
      MessageNode.make({
        nodeId: 1,
        role: "assistant",
        content: "hi there",
        createdAt: 1_700_000_050,
        metadata: null,
      }),
    ],
    promptHistory: [],
  });

interface Node {
  readonly url: string;
  readonly token: string;
  readonly stop: () => void;
  readonly meta: ReturnType<typeof createMetaStore>;
}

/** Boot a node: a real sqlite-backed control plane + meta + convert target. */
const bootNode = async (name: string): Promise<Node> => {
  const dbPath = join(tmp, `${name}.db`);
  const clineDir = join(tmp, `${name}-cline`);
  const meta = createMetaStore(join(tmp, `${name}-meta.json`));
  const layer = controlPlaneLayer({ agents: [], defaultAgentId: "devin" }).pipe(
    Layer.provide(SqliteStorage.layer(dbPath)),
  );
  const runtime = ManagedRuntime.make(layer);
  const plane = await runtime.runPromise(ControlPlane);
  const server = Bun.serve({
    hostname: "127.0.0.1",
    port: 0,
    fetch: createApp(plane, {
      token: `tok-${name}`,
      meta,
      convert: { dbPath, clineDir },
      node: { id: `node_${name}`, name, version: "test" },
      logger: () => {},
    }),
  });
  return {
    url: server.url.origin,
    token: `tok-${name}`,
    meta,
    stop: () => {
      server.stop(true);
      void runtime.dispose();
    },
  };
};

/** Seed a session straight into a node's sqlite store, then project-tag it. */
const seed = async (dbPath: string, session: Session): Promise<void> => {
  const fsLayer = Layer.mergeAll(BunFileSystem.layer, BunPath.layer);
  await Effect.runPromise(
    Conversion.importSession(session).pipe(
      Effect.provide(Layer.mergeAll(SqliteStorage.layer(dbPath), fsLayer)),
    ),
  );
};

const readSse = async (res: Response): Promise<Array<{ event: string; data: unknown }>> => {
  const text = await res.text();
  return text
    .split("\n\n")
    .filter((part) => part.trim() !== "")
    .map((part) => ({
      event: /^event: (.*)$/m.exec(part)?.[1] ?? "",
      data: JSON.parse(/^data: (.*)$/m.exec(part)?.[1] ?? "{}") as unknown,
    }));
};

const nodeA = await bootNode("a");
const nodeB = await bootNode("b");

try {
  // Seed node A: one session in a project.
  await seed(join(tmp, "a.db"), fixture("sess-alpha"));
  const projectA = nodeA.meta.createProject("Boxed");
  nodeA.meta.patch("sess-alpha", { projectIds: [projectA.id], pinned: true });

  // B pulls the project — node-to-node fetch with the source's credential.
  const pull = await fetch(`${nodeB.url}/api/projects/pull`, {
    method: "POST",
    headers: { authorization: `Bearer ${nodeB.token}`, "content-type": "application/json" },
    body: JSON.stringify({
      source: { url: nodeA.url, token: nodeA.token },
      project: projectA.id,
    }),
  });
  assert.equal(pull.status, 200, `pull status: ${pull.status}`);
  const pullFrames = await readSse(pull);
  assert.equal(pullFrames[0]?.event, "start");
  assert.ok(
    pullFrames.some((f) => f.event === "session"),
    "expected session progress",
  );
  const done = pullFrames.at(-1);
  assert.equal(done?.event, "done");
  const summary = done?.data as { imported: unknown[]; truncated: boolean };
  assert.equal(summary.imported.length, 1);
  assert.equal(summary.truncated, false);

  // The project + session + overlay landed on B under the same ids.
  assert.deepEqual(
    nodeB.meta.listProjects().map((p) => p.id),
    [projectA.id],
  );
  const landed = nodeB.meta.of("sess-alpha");
  assert.equal(landed?.projectIds?.[0], projectA.id);
  assert.equal(landed?.pinned, true);

  // Full IR survived the hop.
  const ir = await fetch(`${nodeB.url}/api/sessions/sess-alpha/export`, {
    headers: { authorization: `Bearer ${nodeB.token}` },
  });
  assert.equal(ir.status, 200);
  const { session } = (await ir.json()) as { session: { nodes: unknown[]; title: string } };
  assert.equal(session.nodes.length, 2);
  assert.equal(session.title, "Session sess-alpha");

  // Push the same project B→A: an idempotent update, not a clone.
  const push = await fetch(`${nodeB.url}/api/projects/${projectA.id}/push`, {
    method: "POST",
    headers: { authorization: `Bearer ${nodeB.token}`, "content-type": "application/json" },
    body: JSON.stringify({ target: { url: nodeA.url, token: nodeA.token } }),
  });
  assert.equal(push.status, 200);
  const pushFrames = await readSse(push);
  assert.equal(pushFrames.at(-1)?.event, "done");
  assert.equal(nodeA.meta.listProjects().length, 1, "push re-pulled, not duplicated");

  // Single-session pull: a standalone session on A, pulled onto B over the
  // same node-to-node auth — lands under its source id.
  await seed(join(tmp, "a.db"), fixture("sess-solo"));
  const sessPull = await fetch(`${nodeB.url}/api/sessions/pull`, {
    method: "POST",
    headers: { authorization: `Bearer ${nodeB.token}`, "content-type": "application/json" },
    body: JSON.stringify({
      url: nodeA.url,
      token: nodeA.token,
      sessionId: "sess-solo",
    }),
  });
  assert.equal(sessPull.status, 201, `session pull status: ${sessPull.status}`);
  assert.deepEqual(await sessPull.json(), { id: "sess-solo" });
  assert.equal(nodeB.meta.of("sess-solo")?.agent, "devin");
  const pulledIr = await fetch(`${nodeB.url}/api/sessions/sess-solo/export`, {
    headers: { authorization: `Bearer ${nodeB.token}` },
  });
  assert.equal(pulledIr.status, 200);
  const pulledBody = (await pulledIr.json()) as { session: { nodes: unknown[] } };
  assert.equal(pulledBody.session.nodes.length, 2);

  // Auth: a wrong source token fails as an error frame, not silently.
  const denied = await fetch(`${nodeB.url}/api/projects/pull`, {
    method: "POST",
    headers: { authorization: `Bearer ${nodeB.token}`, "content-type": "application/json" },
    body: JSON.stringify({
      source: { url: nodeA.url, token: "wrong" },
      project: projectA.id,
    }),
  });
  const deniedFrames = await readSse(denied);
  assert.equal(deniedFrames.at(-1)?.event, "error");

  console.log("e2e-transfer: PASS");
} finally {
  nodeA.stop();
  nodeB.stop();
  rmSync(tmp, { recursive: true, force: true });
}

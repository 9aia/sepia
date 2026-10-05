/**
 * `spawnAgent` against a real child process — a minimal NDJSON/JSON-RPC
 * responder stands in for a true ACP agent.
 */
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, expect, test } from "vite-plus/test";
import { spawnAgent } from "../src/spawn.js";

const dir = mkdtempSync(join(tmpdir(), "sepia-acp-spawn-"));
const agentPath = join(dir, "agent.mjs");
writeFileSync(
  agentPath,
  `import { createInterface } from "node:readline";
const rl = createInterface({ input: process.stdin });
rl.on("line", (line) => {
  const msg = JSON.parse(line);
  if (msg.id === undefined) return;
  if (msg.method === "initialize") {
    process.stdout.write(JSON.stringify({
      jsonrpc: "2.0",
      id: msg.id,
      result: {
        protocolVersion: 1,
        agentCapabilities: {
          loadSession: true,
          promptCapabilities: { image: true },
          sessionCapabilities: { list: {} },
        },
      },
    }) + "\\n");
    return;
  }
  process.stdout.write(JSON.stringify({ jsonrpc: "2.0", id: msg.id, result: {} }) + "\\n");
});
`,
);
afterAll(() => rmSync(dir, { recursive: true, force: true }));

test("spawnAgent refuses a spec with no command", async () => {
  await expect(spawnAgent({ id: "x", label: "x", command: [] }, { cwd: "/" })).rejects.toThrow(
    "has no command to spawn",
  );
});

test("spawnAgent fails when the child never exposes stdio", async () => {
  await expect(
    spawnAgent({ id: "x", label: "x", command: ["definitely-not-a-binary-xyz"] }, { cwd: "/" }),
  ).rejects.toThrow();
});

test("spawnAgent initializes against a real NDJSON-speaking child", async () => {
  const connection = await spawnAgent(
    { id: "fake", label: "fake", command: [process.execPath, agentPath] },
    { cwd: dir },
  );
  try {
    expect(connection.capabilities.loadSession).toBe(true);
    expect(connection.capabilities.promptCapabilities.image).toBe(true);
    expect(connection.capabilities.sessionList).toBe(true);
    // stderr wiring works — the tail reports whatever the child printed
    expect(connection.recentStderr()).toEqual([]);
  } finally {
    await connection.close();
  }
});

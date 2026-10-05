/**
 * serve.ts + telemetry.ts — the boot seam. Under node the sqlite driver is
 * stubbed, so `startServer` proves its guard rails and reaches the control
 * plane build before failing; `otelLayer` is constructed for real.
 */
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, expect, test } from "vite-plus/test";
import { startServer } from "../src/serve";
import { otelLayer } from "../src/telemetry";

const tmp = mkdtempSync(join(tmpdir(), "sepia-serve-"));
afterAll(() => rmSync(tmp, { recursive: true, force: true }));

const env = (over = {}) => ({
  dbPath: join(tmp, "sessions.db"),
  port: 0,
  host: "127.0.0.1",
  token: undefined as string | undefined,
  metaPath: join(tmp, "meta.json"),
  home: tmp,
  nodePath: join(tmp, "node.json"),
  nodeName: "test-node",
  serversPath: join(tmp, "servers.json"),
  serversKeyPath: join(tmp, "servers.key"),
  origins: [] as string[],
  ui: { enabled: false, dir: undefined },
  otel: { enabled: false, endpoint: "http://localhost:4318", serviceName: "sepia" },
  ...over,
});

test("startServer refuses a non-loopback bind without a token", async () => {
  await expect(startServer(env({ host: "0.0.0.0" }) as never)).rejects.toThrow(/SEPIA_TOKEN/);
  await expect(startServer(env({ host: "::1", token: "t" }) as never)).rejects.toThrow(
    // loopback but the sqlite stub always fails the control-plane build
    /failed to start/,
  );
});

test("startServer builds agents, repos and the plane until the stubbed store fails", async () => {
  process.env.SEPIA_AGENT_DEVIN_COMMAND = "bun /tmp/agent.mjs";
  try {
    await expect(startServer(env() as never)).rejects.toThrow(/failed to start|bun:sqlite/);
  } finally {
    delete process.env.SEPIA_AGENT_DEVIN_COMMAND;
  }
});

test("otelLayer builds a layer for the collector endpoint", () => {
  const layer = otelLayer({ endpoint: "http://localhost:4318", serviceName: "sepia" });
  expect(layer).toBeDefined();
});

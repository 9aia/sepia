import { describe, expect, it } from "vite-plus/test";
import { resolveAgent } from "../src/AgentRegistry.js";
import { spawnAgent } from "../src/spawn.js";

describe.skipIf(process.env.ACP_IT !== "1")("devin acp integration", () => {
  it("initializes and lists sessions", async () => {
    const connection = await spawnAgent(resolveAgent("devin"), { cwd: process.cwd() });
    try {
      expect(connection.capabilities.loadSession).toBe(true);
      expect(Array.isArray(await connection.listSessions())).toBe(true);
    } finally {
      await connection.close();
    }
  }, 30_000);
});

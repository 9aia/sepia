import { expect, test } from "vite-plus/test";
import { parseEnv } from "../src/env";

test("applies defaults", () => {
  const env = parseEnv({ SEPIA_DB: "/tmp/sepia.db" });

  expect(env.port).toBe(8787);
  expect(env.host).toBe("127.0.0.1");
  expect(env.token).toBeUndefined();
  expect(env.origins).toEqual(["http://localhost:3000", "http://127.0.0.1:3000"]);
});

test("derives the agent URL from the port", () => {
  const env = parseEnv({ SEPIA_DB: "/tmp/sepia.db", PORT: "9000" });

  expect(env.port).toBe(9000);
});

test("rejects a non-numeric PORT", () => {
  expect(() => parseEnv({ SEPIA_DB: "/tmp/sepia.db", PORT: "abc" })).toThrow(/PORT/);
});

test("rejects an out-of-range PORT", () => {
  expect(() => parseEnv({ SEPIA_DB: "/tmp/sepia.db", PORT: "0" })).toThrow(/PORT/);
  expect(() => parseEnv({ SEPIA_DB: "/tmp/sepia.db", PORT: "70000" })).toThrow(/PORT/);
});

test("rejects an empty SEPIA_DB", () => {
  expect(() => parseEnv({ SEPIA_DB: "" })).toThrow(/SEPIA_DB/);
});

test("parses SEPIA_ORIGINS and SEPIA_AGENT_URL overrides", () => {
  const env = parseEnv({
    SEPIA_DB: "/tmp/sepia.db",
    SEPIA_ORIGINS: "https://a.example, https://b.example ,",
    SEPIA_AGENT_URL: "https://sepia.example/api/agent",
  });

  expect(env.origins).toEqual(["https://a.example", "https://b.example"]);
});

import { expect, test } from "vite-plus/test";
import { buildChildEnv } from "../src/spawn.js";
import { StderrTail } from "../src/stderr.js";

const spec = { id: "devin", label: "Devin", command: ["devin", "acp"] } as const;

test("child env is built from an allowlist", () => {
  const env = buildChildEnv(
    spec,
    { cwd: "/work" },
    {
      PATH: "/usr/bin",
      HOME: "/home/dev",
      WINDSURF_API_KEY: "key",
      SECRET_TOKEN: "leak",
    },
  );

  expect(env.PATH).toBe("/usr/bin");
  expect(env.HOME).toBe("/home/dev");
  expect(env.WINDSURF_API_KEY).toBe("key");
  expect(env.SECRET_TOKEN).toBeUndefined();
});

test("spec and option env win over the allowlist", () => {
  const env = buildChildEnv(
    { ...spec, env: { LANG: "en_US.UTF-8" } },
    { cwd: "/work", env: { LANG: "C", CUSTOM: "1" } },
    { PATH: "/usr/bin", LANG: "de" },
  );

  expect(env.LANG).toBe("C");
  expect(env.CUSTOM).toBe("1");
  expect(env.PATH).toBe("/usr/bin");
});

test("SEPIA_INHERIT_ENV=1 forwards the whole environment", () => {
  const env = buildChildEnv(
    spec,
    { cwd: "/work" },
    {
      PATH: "/usr/bin",
      SECRET_TOKEN: "kept",
      SEPIA_INHERIT_ENV: "1",
    },
  );

  expect(env.SECRET_TOKEN).toBe("kept");
});

test("keeps a bounded, prefixed ring of the last lines", () => {
  const tail = new StderrTail({ id: "devin" });
  for (let index = 0; index < 105; index += 1) tail.push(`line ${index}\n`);

  const recent = tail.recent();
  expect(recent).toHaveLength(100);
  expect(recent[0]).toBe("[agent:devin] line 5");
  expect(recent[99]).toBe("[agent:devin] line 104");
});

test("truncates long lines to 500 characters", () => {
  const tail = new StderrTail({ id: "devin" });
  tail.push(`${"x".repeat(900)}\n`);

  const [line] = tail.recent();
  expect(line).toBe(`[agent:devin] ${"x".repeat(500)}`);
});

test("only forwards to the sink when debugging", () => {
  const written: string[] = [];
  const quiet = new StderrTail({ id: "devin", sink: (line) => written.push(line) });
  quiet.push("hidden\n");
  expect(written).toEqual([]);

  const loud = new StderrTail({ id: "devin", debug: true, sink: (line) => written.push(line) });
  loud.push("shown\n");
  expect(written).toEqual(["[agent:devin] shown"]);
});

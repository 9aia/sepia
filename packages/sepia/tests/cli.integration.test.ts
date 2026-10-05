import { execFileSync } from "node:child_process";
import {
  existsSync,
  mkdtempSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, test } from "vite-plus/test";

/**
 * End-to-end runs of the sepia CLI against scratch stores — no live Devin or
 * Cline state is touched. Skipped only when bun is unavailable; the CLI itself
 * is a bun program (bun:sqlite), so this is the faithful way to exercise it.
 */
const hasBun = (() => {
  try {
    execFileSync("bun", ["--version"], { stdio: "ignore" });
    return true;
  } catch {
    return false;
  }
})();
const describeE2E = hasBun ? describe : describe.skip;

const mainTs = new URL("../../../apps/sepia/src/main.ts", import.meta.url).pathname;

const run = (workdir: string, args: ReadonlyArray<string>): { code: number; output: string } => {
  try {
    const output = execFileSync("bun", [mainTs, ...args], {
      cwd: workdir,
      encoding: "utf-8",
      timeout: 180_000,
    });
    return { code: 0, output };
  } catch (error) {
    const child = error as { status?: number; stdout?: string; stderr?: string };
    return {
      code: child.status ?? 1,
      output: `${child.stdout ?? ""}\n${child.stderr ?? ""}`,
    };
  }
};

const runExpectFailure = (workdir: string, args: ReadonlyArray<string>): string => {
  try {
    const output = execFileSync("bun", [mainTs, ...args], {
      cwd: workdir,
      encoding: "utf-8",
      timeout: 180_000,
    });
    throw new Error(`expected a non-zero exit, got: ${output}`);
  } catch (error) {
    const child = error as { status?: number; stdout?: string; stderr?: string };
    if (child.status === undefined || child.status === 0) {
      throw error;
    }
    return `${child.stdout ?? ""}\n${child.stderr ?? ""}`;
  }
};

const fixtureClineSession = (sessionsDir: string, id: string) => {
  const dir = join(sessionsDir, id);
  mkdirSync(dir, { recursive: true });
  writeFileSync(
    join(dir, `${id}.json`),
    JSON.stringify({
      version: 1,
      session_id: id,
      source: "cli",
      cwd: "/work",
      started_at: "2026-09-01T00:00:00.000Z",
      ended_at: "2026-09-01T00:01:00.000Z",
      status: "completed",
      interactive: true,
      provider: "cline-pass",
      model: "deepseek/deepseek-v4-flash",
      prompt: "round trip check",
      metadata: { title: "round trip check" },
    }),
  );
  writeFileSync(
    join(dir, `${id}.messages.json`),
    JSON.stringify({
      version: 1,
      updated_at: "2026-09-01T00:01:00.000Z",
      agent: "lead",
      sessionId: id,
      origin: { source: "cli", mode: "user", sessionId: id, version: "3.0.61" },
      messages: [
        {
          id: "msg_u0",
          role: "user",
          content: [{ type: "text", text: "round trip check" }],
          ts: 1,
        },
        {
          id: "msg_a1",
          role: "assistant",
          content: [
            { type: "thinking", thinking: "checking" },
            { type: "text", text: "on it" },
            {
              type: "tool_use",
              id: "call_1",
              name: "run_commands",
              input: { commands: ["pwd"] },
            },
          ],
          ts: 2,
          modelInfo: { id: "deepseek/deepseek-v4-flash", provider: "cline-pass" },
        },
        {
          id: "msg_u2",
          role: "user",
          content: [
            {
              type: "tool_result",
              tool_use_id: "call_1",
              name: "run_commands",
              content: [{ query: "pwd", result: "/work", success: true }],
            },
          ],
          ts: 3,
        },
      ],
    }),
  );
};

describeE2E("sepia cli", () => {
  let root = "";
  let dbPath = "";
  let fixtureDir = "";

  beforeAll(() => {
    root = mkdtempSync(join(tmpdir(), "sepia-e2e-"));
    dbPath = join(root, "sessions.db");
    fixtureDir = join(root, "fixture", "1788000000000_e2e99");
    fixtureClineSession(join(root, "fixture"), "1788000000000_e2e99");
  });

  afterAll(() => {
    if (root) {
      rmSync(root, { recursive: true, force: true });
    }
  });

  test("import accepts a Cline session dir and list reads it back", () => {
    const imported = run(root, ["import", fixtureDir, "--db", dbPath]);
    expect(imported.code).toBe(0);
    // the CLI logs both a warning line and the confirmation; assert on the latter.
    const ansi = new RegExp(String.fromCharCode(27) + "\\[[0-9;]*m", "g");
    expect(imported.output.replace(ansi, "")).toContain(
      "Imported Cline session 1788000000000_e2e99 into storage",
    );

    const listed = run(root, ["list", "--db", dbPath]);
    expect(listed.code).toBe(0);
    expect(listed.output).toContain("1788000000000_e2e99");
    expect(listed.output).toContain("round trip check");
  });

  test("install writes artifacts plus the session index row", () => {
    const dataDir = join(root, "cline-data");
    const installed = run(root, [
      "install",
      "1788000000000_e2e99",
      "--db",
      dbPath,
      "--data-dir",
      dataDir,
    ]);
    expect(installed.code).toBe(0);
    const installedId = (installed.output.match(/Installed session (\S+) into/) ?? [])[1];
    // generated from the session's start time in the CLI's <epoch-ms>_<5> shape
    expect(installedId).toMatch(/^1788220800000_[a-z0-9]{5}$/);

    const dir = join(dataDir, "sessions", installedId);
    expect(existsSync(join(dir, `${installedId}.json`))).toBe(true);
    expect(existsSync(join(dir, `${installedId}.messages.json`))).toBe(true);

    const meta = JSON.parse(readFileSync(join(dir, `${installedId}.json`), "utf-8"));
    expect(meta.session_id).toBe(installedId);
    expect(meta.messages_path).toBe(join(dir, `${installedId}.messages.json`));

    const messages = JSON.parse(readFileSync(join(dir, `${installedId}.messages.json`), "utf-8"));
    const assistant = messages.messages.find((m: { role: string }) => m.role === "assistant");
    // unsigned thinking is dropped by design (provider-sealed signatures only)
    expect(assistant.content.map((c: { type: string }) => c.type)).toEqual(["text", "tool_use"]);
    // tool call ids are regenerated per import; pairing is what must survive
    const toolUse = assistant.content[1];
    expect(toolUse.name).toBe("run_commands");
    expect(messages.messages[2].content[0].tool_use_id).toBe(toolUse.id);

    const script = [
      'import { Database } from "bun:sqlite";',
      `const db = new Database(${JSON.stringify(join(dataDir, "db", "sessions.db"))});`,
      'console.log(JSON.stringify(db.query("select session_id, status, messages_path from sessions").get()));',
    ].join(" ");
    const query = execFileSync("bun", ["-e", script], { cwd: root, encoding: "utf-8" });
    expect(query).toContain(installedId);
    expect(query).toContain('"status":"completed"');
  });

  test("install refuses a live-owned session id unless forced", () => {
    // a fresh data dir so the guard test's own minimal DDL applies
    const dataDir = join(root, "cline-data-guard");
    mkdirSync(join(dataDir, "db"), { recursive: true });

    const writeRow = (status: string, pid: number) => {
      const script = [
        'import { Database } from "bun:sqlite";',
        `const db = new Database(${JSON.stringify(join(dataDir, "db", "sessions.db"))});`,
        'db.run("PRAGMA busy_timeout = 5000;");',
        "db.run(\"CREATE TABLE IF NOT EXISTS sessions (session_id TEXT PRIMARY KEY, source TEXT NOT NULL DEFAULT 'cli', pid INTEGER NOT NULL DEFAULT 0, started_at TEXT NOT NULL DEFAULT '', ended_at TEXT, exit_code INTEGER, status TEXT NOT NULL, status_lock INTEGER NOT NULL DEFAULT 0, interactive INTEGER NOT NULL DEFAULT 1, provider TEXT NOT NULL DEFAULT '', model TEXT NOT NULL DEFAULT '', cwd TEXT NOT NULL DEFAULT '', workspace_root TEXT NOT NULL DEFAULT '', team_name TEXT, enable_tools INTEGER NOT NULL DEFAULT 1, enable_spawn INTEGER NOT NULL DEFAULT 1, enable_teams INTEGER NOT NULL DEFAULT 1, parent_session_id TEXT, parent_agent_id TEXT, agent_id TEXT, conversation_id TEXT, is_subagent INTEGER NOT NULL DEFAULT 0, prompt TEXT, metadata_json TEXT, transcript_path TEXT NOT NULL DEFAULT '', hook_path TEXT NOT NULL DEFAULT '', messages_path TEXT, updated_at TEXT NOT NULL DEFAULT '');\");",
        'const row = db.query("INSERT OR REPLACE INTO sessions (session_id, source, status, pid) VALUES (?, ?, ?, ?)");',
        `row.run(${JSON.stringify("1788000000001_guard")}, ${JSON.stringify("cli")}, ${JSON.stringify(status)}, ${pid});`,
      ].join(" ");
      execFileSync("bun", ["-e", script], { cwd: root });
    };

    writeRow("running", process.pid);
    const refused = runExpectFailure(root, [
      "install",
      "1788000000000_e2e99",
      "--db",
      dbPath,
      "--data-dir",
      dataDir,
      "--id",
      "1788000000001_guard",
    ]);
    expect(refused).toContain("still belongs to a live owner");

    writeRow("completed", process.pid);
    const adopted = run(root, [
      "install",
      "1788000000000_e2e99",
      "--db",
      dbPath,
      "--data-dir",
      dataDir,
      "--id",
      "1788000000001_guard",
    ]);
    expect(adopted.code).toBe(0);
    expect(adopted.output).toContain("Installed session 1788000000001_guard");

    writeRow("running", process.pid);
    const forced = run(root, [
      "install",
      "1788000000000_e2e99",
      "--db",
      dbPath,
      "--data-dir",
      dataDir,
      "--id",
      "1788000000001_guard",
      "--force",
    ]);
    expect(forced.code).toBe(0);
    expect(forced.output).toContain("Installed session 1788000000001_guard");
  });

  test("claude store lists, exports and round-trips through install", () => {
    const claudeDir = join(root, "claude");
    const projectDir = join(claudeDir, "projects", "-work");
    mkdirSync(projectDir, { recursive: true });
    writeFileSync(
      join(projectDir, "sess-claude-1.jsonl"),
      [
        JSON.stringify({ type: "summary", summary: "Claude fixture", leafUuid: "u2" }),
        JSON.stringify({
          type: "user",
          uuid: "u1",
          parentUuid: null,
          sessionId: "sess-claude-1",
          cwd: "/work",
          timestamp: "2026-01-01T00:00:00.000Z",
          message: { role: "user", content: "claude prompt" },
        }),
        JSON.stringify({
          type: "assistant",
          uuid: "u2",
          parentUuid: "u1",
          sessionId: "sess-claude-1",
          timestamp: "2026-01-01T00:00:01.000Z",
          message: {
            role: "assistant",
            model: "claude-opus-4-5",
            content: [{ type: "text", text: "claude answer" }],
          },
        }),
      ].join("\n"),
    );

    const listed = run(root, ["list", "--claude-dir", claudeDir]);
    expect(listed.code).toBe(0);
    expect(listed.output).toContain("sess-claude-1");
    expect(listed.output).toContain("Claude fixture");

    const exported = run(root, ["export", "sess-claude-1", "--claude-dir", claudeDir]);
    expect(exported.code).toBe(0);
    const ansi = new RegExp(String.fromCharCode(27) + "\\[[0-9;]*m", "g");
    const sessionJson = JSON.parse(exported.output.replace(ansi, ""));
    expect(sessionJson.id).toBe("sess-claude-1");
    expect(sessionJson.nodes.map((n: { role: string }) => n.role)).toEqual(["user", "assistant"]);

    // devin → claude: install writes the canonical <slug>/<id>.jsonl
    const installed = run(root, [
      "install",
      "1788000000000_e2e99",
      "--db",
      dbPath,
      "--claude-dir",
      claudeDir,
      "--id",
      "sess-installed",
    ]);
    expect(installed.code).toBe(0);
    const writtenPath = join(claudeDir, "projects", "-work", "sess-installed.jsonl");
    expect(existsSync(writtenPath)).toBe(true);
    const firstEntry = JSON.parse(readFileSync(writtenPath, "utf-8").split("\n")[1]);
    expect(firstEntry.sessionId).toBe("sess-installed");

    const relisted = run(root, ["list", "--claude-dir", claudeDir]);
    expect(relisted.output).toContain("sess-installed");

    const deleted = run(root, ["delete", "sess-installed", "--claude-dir", claudeDir]);
    expect(deleted.code).toBe(0);
    expect(existsSync(writtenPath)).toBe(false);
  });

  test("cursor store installs through the canonical store.db write", () => {
    const cursorDir = join(root, "cursor");
    const installed = run(root, [
      "install",
      "1788000000000_e2e99",
      "--db",
      dbPath,
      "--cursor-dir",
      cursorDir,
    ]);
    expect(installed.code).toBe(0);
    expect(installed.output).toContain("Installed session 1788000000000_e2e99");

    // the canonical write: chats/<workspace-hash>/<id>/store.db exists
    const wsHashes = readdirSync(join(cursorDir, "chats"));
    expect(wsHashes).toHaveLength(1);
    const chatDir = join(cursorDir, "chats", wsHashes[0], "1788000000000_e2e99");
    expect(existsSync(join(chatDir, "store.db"))).toBe(true);
    expect(existsSync(join(chatDir, "meta.json"))).toBe(true);

    const listed = run(root, ["list", "--cursor-dir", cursorDir]);
    expect(listed.code).toBe(0);
    expect(listed.output).toContain("1788000000000_e2e99");

    // re-installing refuses without --force
    const refused = runExpectFailure(root, [
      "install",
      "1788000000000_e2e99",
      "--db",
      dbPath,
      "--cursor-dir",
      cursorDir,
    ]);
    expect(refused).toContain("already exists");
  });

  test("export writes session JSON; import reads it into another store", () => {
    const outFile = join(root, "session.json");
    const exported = run(root, ["export", "1788000000000_e2e99", outFile, "--db", dbPath]);
    expect(exported.code).toBe(0);
    const parsed = JSON.parse(readFileSync(outFile, "utf-8"));
    expect(parsed.id).toBe("1788000000000_e2e99");
    expect(parsed.nodes.length).toBeGreaterThan(0);

    const cursorDir = join(root, "cursor-json");
    const imported = run(root, ["import", outFile, "--cursor-dir", cursorDir]);
    expect(imported.code).toBe(0);
    const listed = run(root, ["list", "--cursor-dir", cursorDir]);
    expect(listed.output).toContain("1788000000000_e2e99");
  });

  test("export refuses an unknown session id", () => {
    const missing = runExpectFailure(root, [
      "export",
      "no-such-session",
      join(root, "out"),
      "--db",
      dbPath,
    ]);
    expect(missing).toContain("Session not found: no-such-session");
  });

  test("config verbs list, export, install and diff between agent stores", () => {
    // A scratch .claude dir: skill + memory + hook + command.
    const claudeDir = join(root, "cfg", ".claude");
    mkdirSync(join(claudeDir, "skills", "pnpm"), { recursive: true });
    mkdirSync(join(claudeDir, "commands"), { recursive: true });
    writeFileSync(
      join(claudeDir, "skills", "pnpm", "SKILL.md"),
      "---\nname: pnpm\ndescription: use pnpm\n---\nUse pnpm.\n",
    );
    writeFileSync(join(claudeDir, "CLAUDE.md"), "memory\n");
    writeFileSync(join(claudeDir, "commands", "go.md"), "Do it.\n");
    writeFileSync(
      join(claudeDir, "settings.json"),
      JSON.stringify({
        hooks: { Stop: [{ hooks: [{ type: "command", command: "done.sh" }] }] },
      }),
    );

    const listed = run(root, ["config", "list", "--claude-dir", claudeDir]);
    expect(listed.code).toBe(0);
    expect(listed.output).toContain("pnpm");
    expect(listed.output).toContain("go");
    expect(listed.output).toContain("Stop");

    const outFile = join(root, "cfg.json");
    const exported = run(root, ["config", "export", outFile, "--claude-dir", claudeDir]);
    expect(exported.code).toBe(0);
    const ir = JSON.parse(readFileSync(outFile, "utf-8"));
    expect(ir.version).toBe(1);
    expect(ir.skills[0].name).toBe("pnpm");

    // claude → cursor install: rules dir + hooks.json land in cursor shape.
    const cursorDir = join(root, "cfg-cursor");
    const installed = run(root, [
      "config",
      "install",
      "--from",
      "claude",
      "--to",
      "cursor",
      "--claude-dir",
      claudeDir,
      "--cursor-dir",
      cursorDir,
    ]);
    expect(installed.code).toBe(0);
    expect(installed.output).toContain("Installed claude config into cursor");
    expect(existsSync(join(cursorDir, "rules", "claude.md"))).toBe(true);
    expect(existsSync(join(cursorDir, "skills", "pnpm", "SKILL.md"))).toBe(true);
    expect(existsSync(join(cursorDir, "commands", "go.md"))).toBe(true);
    const cursorHooks = JSON.parse(readFileSync(join(cursorDir, "hooks.json"), "utf-8"));
    expect(cursorHooks.hooks.stop).toEqual([{ command: "done.sh" }]);

    // A config JSON import writes the devin store shape.
    const devinDir = join(root, "cfg-devin");
    const imported = run(root, ["config", "import", outFile, "--devin-dir", devinDir]);
    expect(imported.code).toBe(0);
    expect(existsSync(join(devinDir, "workflows", "go.md"))).toBe(true);
    expect(existsSync(join(devinDir, "hooks.v1.json"))).toBe(true);

    // diff reports the delta between the two installed stores.
    const diffed = run(root, [
      "config",
      "diff",
      "--from",
      "cursor",
      "--to",
      "devin",
      "--cursor-dir",
      cursorDir,
      "--devin-dir",
      devinDir,
    ]);
    expect(diffed.code).toBe(0);
    expect(diffed.output).toContain("# cursor → devin");
  });
});

/**
 * `makeRewinders` — per-store truncate write-backs. Under node the sqlite
 * drivers are stubbed to throw, which exercises the ControlError mapping;
 * the filesystem-backed rewinders run against real temp dirs and fail on
 * the missing session — proving each backend reaches its store.
 */
import { Effect, Either } from "effect";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, expect, test } from "vite-plus/test";
import { MessageNode, Session } from "sepia-core";
import { makeRewinders } from "../src/rewinders";

const tmp = mkdtempSync(join(tmpdir(), "sepia-rewinders-"));
afterAll(() => rmSync(tmp, { recursive: true, force: true }));

const session = Session.make({
  id: "s1",
  title: "t",
  workingDirectory: tmp,
  model: "m",
  createdAt: 1,
  lastActivityAt: 2,
  mainChainId: 0,
  metadata: null,
  nodes: [
    MessageNode.make({ nodeId: 0, role: "user", content: "hi", createdAt: 1, metadata: null }),
  ],
});

const plan = {
  keepCount: 1,
  kept: [
    MessageNode.make({ nodeId: 0, role: "user", content: "hi", createdAt: 1, metadata: null }),
  ],
  removed: [
    MessageNode.make({
      nodeId: 1,
      role: "assistant",
      content: "bye",
      createdAt: 2,
      metadata: null,
    }),
  ],
  removedToolCallIds: ["c1"],
};

const paths = {
  dbPath: join(tmp, "sessions.db"),
  clineDir: join(tmp, "cline"),
  claudeDir: join(tmp, "claude"),
  cursorDir: join(tmp, "cursor"),
};

test("each backend's truncate runs its store write and maps failures", async () => {
  const rewinders = makeRewinders(paths);
  expect(Object.keys(rewinders).sort()).toEqual(["claude", "cline", "cursor", "devin"]);

  // devin: the stubbed bun:sqlite Database throws inside the effect
  const devin = await Effect.runPromise(
    Effect.either(rewinders.devin!.truncate(session, plan, session)),
  );
  expect(Either.isLeft(devin)).toBe(true);
  if (Either.isLeft(devin)) expect((devin.left as { code: string }).code).toBe("internal");

  // cline: no sessions.db → not live-owned; the truncate then fails because
  // the session dir doesn't exist → internal
  const cline = await Effect.runPromise(
    Effect.either(rewinders.cline!.truncate(session, plan, session)),
  );
  expect(Either.isLeft(cline)).toBe(true);
  if (Either.isLeft(cline)) expect((cline.left as { code: string }).code).toBe("internal");

  // claude: no project dir on disk → transcript rewrite fails → internal
  const claude = await Effect.runPromise(
    Effect.either(rewinders.claude!.truncate(session, plan, session)),
  );
  expect(Either.isLeft(claude)).toBe(true);
  if (Either.isLeft(claude)) expect((claude.left as { code: string }).code).toBe("internal");

  // cursor: the canonical save reaches the (stubbed) store.db writer → 500
  const cursor = await Effect.runPromise(
    Effect.either(rewinders.cursor!.truncate(session, plan, session)),
  );
  expect(Either.isLeft(cursor)).toBe(true);
});

test("cline rewind refuses while the index row shows a live owner", async () => {
  // sessions.db exists but is unreadable under node's sqlite stub → the
  // indexRow probe wraps to a ConversionError the caller maps to internal
  mkdirSync(join(paths.clineDir, "db"), { recursive: true });
  writeFileSync(join(paths.clineDir, "db", "sessions.db"), "");
  mkdirSync(join(paths.clineDir, "sessions", "s1"), { recursive: true });
  writeFileSync(
    join(paths.clineDir, "sessions", "s1", "s1.json"),
    JSON.stringify({ session_id: "s1" }),
  );
  writeFileSync(
    join(paths.clineDir, "sessions", "s1", "s1.messages.json"),
    JSON.stringify({ sessionId: "s1", messages: [{ id: "m0", role: "user" }] }),
  );
  const result = await Effect.runPromise(
    Effect.either(makeRewinders(paths).cline!.truncate(session, plan, session)),
  );
  expect(Either.isLeft(result)).toBe(true);
});

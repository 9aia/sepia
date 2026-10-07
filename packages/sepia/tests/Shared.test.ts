import { Option } from "effect";
import { describe, expect, test } from "vite-plus/test";
import { MessageNode, Session, StorageError, ToolCall } from "../src/Domain.js";
import { needsMigration, REQUIRED_TABLES } from "../src/Storage.js";
import * as Shared from "../src/Shared.js";

const node = (over: Partial<Parameters<typeof MessageNode.make>[0]> = {}): MessageNode =>
  MessageNode.make({
    nodeId: 0,
    role: "assistant",
    content: "",
    createdAt: 1_700_000_000,
    metadata: null,
    ...over,
  });

describe("tool outcome folding", () => {
  const call = () => ToolCall.make({ id: "c1", name: "exec", arguments: { command: "ls" } });

  test("toolNodeOutcomes collects role:tool results by call id", () => {
    const nodes = [
      node({ role: "user", content: "go" }),
      node({ toolCalls: [call()] }),
      node({
        role: "tool",
        toolCallId: Option.some("c1"),
        toolResult: Option.some({ status: "error", exitCode: 2, durationMs: 9 }),
      }),
      node({ role: "tool" }), // no id/result — skipped
    ];
    const outcomes = Shared.toolNodeOutcomes(nodes);
    expect(outcomes.size).toBe(1);
    expect(outcomes.get("c1")?.status).toBe("error");
  });

  test("applyToolCallOutcomes stamps status/exitCode/durationMs onto the call", () => {
    const nodes = [
      node({ toolCalls: [call()] }),
      node({
        role: "tool",
        toolCallId: Option.some("c1"),
        toolResult: Option.some({ status: "success", exitCode: 0, durationMs: 12 }),
      }),
    ];
    const out = Shared.applyToolCallOutcomes(nodes, Shared.toolNodeOutcomes(nodes));
    const tc = out[0]?.toolCalls[0];
    expect(tc && Option.getOrUndefined(tc.status)).toBe("success");
    expect(tc && Option.getOrUndefined(tc.exitCode)).toBe(0);
    expect(tc && Option.getOrUndefined(tc.durationMs)).toBe(12);
  });

  test("empty outcome map returns nodes untouched", () => {
    const nodes = [node({ toolCalls: [call()] })];
    expect(Shared.applyToolCallOutcomes(nodes, new Map())).toBe(nodes);
  });
});

describe("canonical import defaults", () => {
  test("defaultSessionMetadata / defaultCogsJson produce usable payloads", () => {
    const meta = Shared.defaultSessionMetadata();
    expect(meta.response_dimensions.length).toBeGreaterThan(0);
    expect(JSON.parse(Shared.defaultCogsJson())).toBeInstanceOf(Array);
  });
});

describe("checkpoint metadata convention", () => {
  test("checkpointsFromMetadata reads sepia/checkpoints and drops malformed entries", () => {
    const meta = {
      [Shared.SESSION_CHECKPOINTS_KEY]: [
        { ref: "cp1", createdAt: 1.5, runCount: 3, kind: "file_history" },
        { ref: "missing-createdAt" },
        "not-an-object",
        null,
      ],
    };
    const cps = Shared.checkpointsFromMetadata(meta);
    expect(cps).toEqual([{ ref: "cp1", createdAt: 1.5, runCount: 3, kind: "file_history" }]);
    expect(Shared.checkpointsFromMetadata(null)).toEqual([]);
    expect(Shared.checkpointsFromMetadata({ other: true })).toEqual([]);
  });
});

describe("project-dir slugs", () => {
  test("decodeProjectDir reverses the flatten for common paths", () => {
    expect(Shared.decodeProjectDir("-home-me-proj")).toBe("/home/me/proj");
    expect(Shared.decodeProjectDir("bare")).toBe("/bare");
  });

  test("encodeProjectDir flattens non-alphanumerics", () => {
    expect(Shared.encodeProjectDir("/home/me/proj")).toBe("-home-me-proj");
    expect(Shared.encodeProjectDir("/my dir/x")).toBe("-my-dir-x");
  });
});

describe("storage port helpers", () => {
  test("needsMigration is true only when a required table is missing", () => {
    expect(needsMigration(new Set(REQUIRED_TABLES))).toBe(false);
    expect(needsMigration(new Set(["sessions"]))).toBe(true);
    expect(needsMigration(new Set())).toBe(true);
  });

  test("SessionRepository tag is usable as an Effect service", async () => {
    const { Effect, Layer } = await import("effect");
    const { SessionRepository } = await import("../src/Storage.js");
    const stub = {
      save: () => Effect.void,
      getById: () => Effect.succeed(Option.none()),
      list: () => Effect.succeed([]),
      delete: () => Effect.void,
      hasSession: () => Effect.succeed(false),
    };
    const listed = await Effect.runPromise(
      Effect.flatMap(SessionRepository, (r) => r.list()).pipe(
        Effect.provide(Layer.succeed(SessionRepository, stub)),
      ),
    );
    expect(listed).toEqual([]);
  });

  test("StorageError carries its message", () => {
    const err = new StorageError({ message: "nope" });
    expect(err.message).toBe("nope");
    expect(err._tag).toBe("StorageError");
  });
});

describe("Session schema shape (Domain)", () => {
  test("minimal session decodes with defaults", () => {
    const s = Session.make({
      id: "x",
      title: "t",
      workingDirectory: "/w",
      model: "m",
      createdAt: 1,
      lastActivityAt: 2,
      mainChainId: 0,
      shellLastSeenIndex: 0,
      cogsJson: "[]",
      workspaceDirs: "[]",
      hidden: 0,
      metadata: null,
      nodes: [],
      promptHistory: [],
    });
    expect(s.backendType).toBe("windsurf");
    expect(s.agentMode).toBe("accept-edits");
    expect(s.cogsJson).toBe("[]");
    expect(s.checkpoints).toEqual([]);
    expect(s.hidden).toBe(0);
  });
});

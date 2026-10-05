import { describe, expect, it } from "vite-plus/test";
import { join } from "node:path";
import {
  diffsForPath,
  fileHistorySnapshot,
  planPathRestore,
  resolveWorkspacePath,
} from "../src/Restore.js";
import { MessageNode, Session, ToolCall } from "../src/Domain.js";

const CWD = "/repo/workspace";

const sessionWith = (
  calls: ReadonlyArray<{
    id: string;
    diffs: ReadonlyArray<{ path: string; oldText?: string; newText?: string }>;
  }>,
  cwd = CWD,
): Session =>
  Session.make({
    id: "s1",
    title: "s1",
    workingDirectory: cwd,
    model: "m",
    createdAt: 0,
    lastActivityAt: 0,
    mainChainId: 0,
    metadata: null,
    nodes: calls.map((call, index) =>
      MessageNode.make({
        nodeId: index,
        role: "assistant",
        content: "",
        createdAt: index,
        metadata: null,
        toolCalls: [
          ToolCall.make({
            id: call.id,
            name: "edit",
            arguments: {},
            diffs: call.diffs.map((d) => ({ ...d })),
          }),
        ],
      }),
    ),
  });

describe("resolveWorkspacePath", () => {
  it("resolves relative paths under the working directory", () => {
    expect(resolveWorkspacePath(CWD, "src/a.ts")).toBe(join(CWD, "src/a.ts"));
  });

  it("keeps absolute paths inside the directory", () => {
    expect(resolveWorkspacePath(CWD, `${CWD}/src/a.ts`)).toBe(`${CWD}/src/a.ts`);
  });

  it("accepts the directory itself", () => {
    expect(resolveWorkspacePath(CWD, ".")).toBe(resolveWorkspacePath(CWD, CWD));
  });

  it("rejects escapes", () => {
    expect(resolveWorkspacePath(CWD, "../outside")).toBeNull();
    expect(resolveWorkspacePath(CWD, "/etc/passwd")).toBeNull();
    expect(resolveWorkspacePath(CWD, "a/../../outside")).toBeNull();
    expect(resolveWorkspacePath("/repo/other", `${CWD}/src/a.ts`)).toBeNull();
  });

  it("rejects empty and blank paths", () => {
    expect(resolveWorkspacePath(CWD, "")).toBeNull();
    expect(resolveWorkspacePath(CWD, "   ")).toBeNull();
  });
});

describe("diffsForPath", () => {
  const session = sessionWith([
    { id: "c1", diffs: [{ path: "a.ts", oldText: "0", newText: "1" }] },
    {
      id: "c2",
      diffs: [
        { path: `${CWD}/a.ts`, oldText: "1", newText: "2" },
        { path: "b.ts", oldText: "x", newText: "y" },
      ],
    },
    { id: "c3", diffs: [] },
  ]);

  it("collects diffs in session order across calls", () => {
    const diffs = diffsForPath(session, "a.ts");
    expect(diffs.map((d) => d.toolCallId)).toEqual(["c1", "c2"]);
    expect(diffs.map((d) => d.diff.newText)).toEqual(["1", "2"]);
  });

  it("matches absolute and relative spellings of the same path", () => {
    expect(diffsForPath(session, `${CWD}/a.ts`)).toHaveLength(2);
    expect(diffsForPath(session, `${CWD}/b.ts`)).toHaveLength(1);
  });

  it("returns nothing for untouched paths", () => {
    expect(diffsForPath(session, "c.ts")).toEqual([]);
  });
});

describe("planPathRestore", () => {
  it("restores whole-file diffs to the state before the session touched the file", () => {
    const session = sessionWith([
      { id: "c1", diffs: [{ path: "a.ts", oldText: "v0", newText: "v1" }] },
      { id: "c2", diffs: [{ path: "a.ts", oldText: "v1", newText: "v2" }] },
    ]);
    expect(planPathRestore(session, "a.ts", "v2")).toEqual({
      kind: "write",
      path: "a.ts",
      content: "v0",
    });
  });

  it("reverts only the named call when toolCallId is given", () => {
    const session = sessionWith([
      { id: "c1", diffs: [{ path: "a.ts", oldText: "v0", newText: "v1" }] },
      { id: "c2", diffs: [{ path: "a.ts", oldText: "v1", newText: "v2" }] },
    ]);
    // Reverting c2 against the current file yields c1's after-state.
    expect(planPathRestore(session, "a.ts", "v2", "c2")).toEqual({
      kind: "write",
      path: "a.ts",
      content: "v1",
    });
    // Reverting c1 while the file holds v2 fails cleanly — the recorded
    // after-state isn't there anymore.
    expect(planPathRestore(session, "a.ts", "v2", "c1")).toEqual({
      kind: "skip",
      path: "a.ts",
      reason: "file no longer contains the recorded after-state",
    });
  });

  it("reports unknown calls", () => {
    const session = sessionWith([
      { id: "c1", diffs: [{ path: "a.ts", oldText: "v0", newText: "v1" }] },
    ]);
    expect(planPathRestore(session, "a.ts", "v1", "nope")).toEqual({
      kind: "skip",
      path: "a.ts",
      reason: "no recorded change for this path in call nope",
    });
  });

  it("reports paths with no recorded diff", () => {
    const session = sessionWith([{ id: "c1", diffs: [] }]);
    expect(planPathRestore(session, "a.ts", "x")).toEqual({
      kind: "skip",
      path: "a.ts",
      reason: "no recorded change for this path",
    });
  });

  it("deletes a file the session created when it is untouched since", () => {
    const session = sessionWith([
      { id: "c1", diffs: [{ path: "new.ts", newText: "fresh content" }] },
    ]);
    expect(planPathRestore(session, "new.ts", "fresh content")).toEqual({
      kind: "delete",
      path: "new.ts",
    });
  });

  it("hunk-reverts a snippet inside a drifted file", () => {
    const session = sessionWith([
      { id: "c1", diffs: [{ path: "a.ts", oldText: "fn old()", newText: "fn new()" }] },
    ]);
    const current = "header\nfn new()\nfooter\n";
    expect(planPathRestore(session, "a.ts", current)).toEqual({
      kind: "write",
      path: "a.ts",
      content: "header\nfn old()\nfooter\n",
    });
  });

  it("skips when the recorded after-state matches multiple positions", () => {
    const session = sessionWith([
      { id: "c1", diffs: [{ path: "a.ts", oldText: "x", newText: "dup" }] },
    ]);
    expect(planPathRestore(session, "a.ts", "dup and dup")).toEqual({
      kind: "skip",
      path: "a.ts",
      reason: "the recorded after-state matches multiple positions",
    });
  });

  it("skips mid-fold when an earlier change's after-state is gone", () => {
    const session = sessionWith([
      { id: "c1", diffs: [{ path: "a.ts", oldText: "v0", newText: "v1" }] },
      { id: "c2", diffs: [{ path: "a.ts", oldText: "v1", newText: "v2" }] },
    ]);
    // The file was hand-edited so v2 is gone — the fold cannot reach v0.
    expect(planPathRestore(session, "a.ts", "drifted")).toEqual({
      kind: "skip",
      path: "a.ts",
      reason: "file no longer contains the recorded after-state",
    });
  });

  it("resurrects the recorded before-state when the file was deleted", () => {
    const session = sessionWith([
      { id: "c1", diffs: [{ path: "a.ts", oldText: "v0", newText: "v1" }] },
    ]);
    expect(planPathRestore(session, "a.ts", null)).toEqual({
      kind: "write",
      path: "a.ts",
      content: "v0",
    });
  });

  it("reports unchanged when a created file is already absent", () => {
    const session = sessionWith([{ id: "c1", diffs: [{ path: "new.ts", newText: "content" }] }]);
    expect(planPathRestore(session, "new.ts", null)).toEqual({
      kind: "unchanged",
      path: "new.ts",
    });
  });

  it("restores a file delete-diff when the file is absent", () => {
    const session = sessionWith([
      { id: "c1", diffs: [{ path: "gone.ts", oldText: "whole file" }] },
    ]);
    expect(planPathRestore(session, "gone.ts", null)).toEqual({
      kind: "write",
      path: "gone.ts",
      content: "whole file",
    });
  });

  it("skips a removal revert while the file still exists", () => {
    const session = sessionWith([
      { id: "c1", diffs: [{ path: "a.ts", oldText: "removed block" }] },
    ]);
    expect(planPathRestore(session, "a.ts", "current content")).toEqual({
      kind: "skip",
      path: "a.ts",
      reason: "cannot re-locate where the recorded removal happened",
    });
  });

  it("skips a diff that recorded neither side", () => {
    const session = sessionWith([{ id: "c1", diffs: [{ path: "a.ts" }] }]);
    expect(planPathRestore(session, "a.ts", "x")).toEqual({
      kind: "skip",
      path: "a.ts",
      reason: "the recorded diff carries no content",
    });
  });

  it("round-trips create → edit → delete chains", () => {
    const session = sessionWith([
      { id: "c1", diffs: [{ path: "a.ts", newText: "v1" }] },
      { id: "c2", diffs: [{ path: "a.ts", oldText: "v1", newText: "v2" }] },
      { id: "c3", diffs: [{ path: "a.ts", oldText: "v2" }] },
    ]);
    // File deleted by c3, recreated elsewhere → reverting all three lands at
    // "file never existed".
    expect(planPathRestore(session, "a.ts", null)).toEqual({
      kind: "unchanged",
      path: "a.ts",
    });
    // If it still holds v2... wait, c3 deleted it. The delete diff's after
    // state is absence; a present file means drift.
    expect(planPathRestore(session, "a.ts", "v2")).toEqual({
      kind: "skip",
      path: "a.ts",
      reason: "cannot re-locate where the recorded removal happened",
    });
  });

  it("handles empty after-state content", () => {
    const session = sessionWith([
      { id: "c1", diffs: [{ path: "a.ts", oldText: "stuff", newText: "" }] },
    ]);
    expect(planPathRestore(session, "a.ts", "")).toEqual({
      kind: "write",
      path: "a.ts",
      content: "stuff",
    });
    expect(planPathRestore(session, "a.ts", "other")).toEqual({
      kind: "skip",
      path: "a.ts",
      reason: "the recorded after-state is empty and the file has since changed",
    });
  });

  it("removes a created block inside a file that later grew", () => {
    const session = sessionWith([
      { id: "c1", diffs: [{ path: "a.ts", newText: "created block" }] },
    ]);
    expect(planPathRestore(session, "a.ts", "pre created block post")).toEqual({
      kind: "write",
      path: "a.ts",
      content: "pre  post",
    });
  });

  it("reports unchanged when the fold reproduces the current content", () => {
    const session = sessionWith([
      { id: "c1", diffs: [{ path: "a.ts", oldText: "same", newText: "same" }] },
    ]);
    expect(planPathRestore(session, "a.ts", "same")).toEqual({
      kind: "unchanged",
      path: "a.ts",
    });
  });

  it("normalizes the request path the same way diffs are matched", () => {
    const session = sessionWith([
      { id: "c1", diffs: [{ path: `${CWD}/a.ts`, oldText: "0", newText: "1" }] },
    ]);
    expect(planPathRestore(session, "a.ts", "1")).toEqual({
      kind: "write",
      path: "a.ts",
      content: "0",
    });
  });
});

describe("fileHistorySnapshot", () => {
  const withMetadata = (metadata: unknown): Session =>
    Session.make({
      id: "s1",
      title: "s1",
      workingDirectory: CWD,
      model: "m",
      createdAt: 0,
      lastActivityAt: 0,
      mainChainId: 0,
      metadata,
      nodes: [],
    });

  it("reads the path→backup map for a recorded ref", () => {
    const session = withMetadata({
      fileHistory: {
        sessionId: "claude-1",
        snapshots: {
          "msg-1": {
            at: 1,
            files: {
              [`${CWD}/a.ts`]: { backup: "hash@v1", version: 1 },
              [`${CWD}/gone.ts`]: { backup: null },
              [`${CWD}/junk.ts`]: "not-an-object",
              [`${CWD}/bad.ts`]: { backup: 42 },
            },
          },
        },
      },
    });
    expect(fileHistorySnapshot(session, "msg-1")).toEqual({
      sessionId: "claude-1",
      files: {
        [`${CWD}/a.ts`]: { backup: "hash@v1", version: 1 },
        [`${CWD}/gone.ts`]: { backup: null },
      },
    });
  });

  it("is undefined when the store recorded no map", () => {
    expect(fileHistorySnapshot(withMetadata(null), "msg-1")).toBeUndefined();
    expect(fileHistorySnapshot(withMetadata({ fileHistory: null }), "msg-1")).toBeUndefined();
    expect(
      fileHistorySnapshot(
        withMetadata({ fileHistory: { sessionId: "s", snapshots: {} } }),
        "msg-1",
      ),
    ).toBeUndefined();
    expect(
      fileHistorySnapshot(
        withMetadata({ fileHistory: { sessionId: 42, snapshots: { "msg-1": { files: {} } } } }),
        "msg-1",
      ),
    ).toBeUndefined();
  });

  it("is undefined when the snapshot's files are not a map", () => {
    expect(
      fileHistorySnapshot(
        withMetadata({
          fileHistory: { sessionId: "s", snapshots: { "msg-1": { files: null } } },
        }),
        "msg-1",
      ),
    ).toBeUndefined();
    expect(
      fileHistorySnapshot(
        withMetadata({
          fileHistory: { sessionId: "s", snapshots: { "msg-1": "not-an-object" } },
        }),
        "msg-1",
      ),
    ).toBeUndefined();
  });
});

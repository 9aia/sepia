import { describe, expect, it } from "vite-plus/test";
import {
  diffFence,
  fileDiffView,
  splitLeadingJson,
  stripAnsi,
  toolContentSegments,
  toolSummary,
} from "../lib/toolDisplay";

// Real payload shapes pulled from GET /api/sessions/:id/history on a devin store.

describe("stripAnsi", () => {
  it("removes CSI and OSC sequences", () => {
    expect(stripAnsi("\x1b[32m✓\x1b[39m ok \x1b[2m(dim)\x1b[22m")).toBe("✓ ok (dim)");
    expect(stripAnsi("plain")).toBe("plain");
  });
});

describe("splitLeadingJson", () => {
  it("parses concatenated snapshots", () => {
    const { values, rest } = splitLeadingJson('{"a":1}{"a":1,"b":2}');
    expect(values).toEqual([{ a: 1 }, { a: 1, b: 2 }]);
    expect(rest).toBe("");
  });

  it("parses string values and trailing text", () => {
    const { values, rest } = splitLeadingJson('"hello"tail');
    expect(values).toEqual(["hello"]);
    expect(rest).toBe("tail");
  });

  it("leaves a partial snapshot in rest mid-stream", () => {
    const { values, rest } = splitLeadingJson('{"command":"ls"}{"comm');
    expect(values).toEqual([{ command: "ls" }]);
    expect(rest).toBe('{"comm');
  });

  it("returns no values for plain text", () => {
    const { values, rest } = splitLeadingJson("some output\nmore");
    expect(values).toEqual([]);
    expect(rest).toBe("some output\nmore");
  });
});

describe("toolSummary — exec", () => {
  it("parses the devin exec result envelope (header, ANSI, exit code)", () => {
    const content =
      "Output from command in shell 4070e4:\n\x1b[32m✓\x1b[39m a.test.ts \x1b[2m(8 tests)\x1b[22m\n\n\nExit code: 0";
    const d = toolSummary("exec", undefined, content);
    expect(d.category).toBe("exec");
    expect(d.label).toBe("Ran command");
    const code = d.segments.find((s) => s.kind === "code");
    expect(code?.kind === "code" && code.text).toBe("✓ a.test.ts (8 tests)");
    const exit = d.segments.find((s) => s.kind === "note" && s.text.startsWith("exit"));
    expect(exit?.kind === "note" && exit.text).toBe("exit 0");
    expect(exit?.kind === "note" && exit.error).toBeUndefined();
  });

  it("marks non-zero exits as error notes", () => {
    const d = toolSummary(
      "execute",
      undefined,
      "Output from command in shell x:\nboom\n\n\nExit code: 2",
    );
    const exit = d.segments.find((s) => s.kind === "note" && s.text === "exit 2");
    expect(exit?.kind === "note" && exit.error).toBe(true);
  });

  it("surfaces the command from live args", () => {
    const d = toolSummary("Ran command", '{"command":"git status"}', "");
    expect(d.label).toBe("Ran command");
    expect(d.detail).toBe("git status");
    expect(d.segments[0]).toEqual({ kind: "command", text: "git status" });
  });

  it("surfaces the command from stored args alongside the result envelope", () => {
    // The history row's `args` is the IR ToolCall.arguments, JSON-encoded —
    // the same string shape a live row accumulates.
    const content = "Output from command in shell 4070e4:\napps/web/src/lib/api.ts\n\nExit code: 0";
    const d = toolSummary("execute", '{"command":"rg -n api apps/web","timeout":40000}', content);
    expect(d.label).toBe("Ran command");
    expect(d.detail).toBe("rg -n api apps/web");
    expect(d.segments[0]).toEqual({ kind: "command", text: "rg -n api apps/web" });
    const code = d.segments.find((s) => s.kind === "code");
    expect(code?.kind === "code" && code.text).toBe("apps/web/src/lib/api.ts");
  });

  it("extracts the command from alternate arg field names", () => {
    expect(toolSummary("execute", '{"cmd":"ls -la"}', "").detail).toBe("ls -la");
    expect(toolSummary("bash", '{"shell":"pwd"}', "").detail).toBe("pwd");
    expect(toolSummary("exec", '{"input":{"command":"make test"}}', "").detail).toBe("make test");
    expect(toolSummary("exec", '{"params":{"cmd":"id"}}', "").detail).toBe("id");
  });

  it("shows only the first line of a multi-line command as the detail", () => {
    const d = toolSummary("exec", '{"command":"cd /repo && npm test\\nP=$!"}', "");
    expect(d.detail).toBe("cd /repo && npm test");
    expect(d.segments[0]).toEqual({ kind: "command", text: "cd /repo && npm test\nP=$!" });
  });

  it("details get_output/kill_shell by shell id when no command was recorded", () => {
    const out = toolSummary("get_output", '{"shell_id":"a6c502","timeout":20000}', "ok");
    expect(out.label).toBe("Read shell output");
    expect(out.detail).toBe("shell a6c502");
    expect(out.segments.some((s) => s.kind === "command")).toBe(false);

    const kill = toolSummary("kill_shell", '{"shell_id":"632775"}', "");
    expect(kill.label).toBe("Killed shell");
    expect(kill.detail).toBe("shell 632775");
  });

  it("keeps truncation notices as notes", () => {
    const content =
      "Output from command in shell 9a8503:\n… (6 lines truncated)\n c8b6e5d..4b592e3  project -> project\n\nExit code: 0\n\n`| tail -1` was parsed out (1 of 7 total lines shown).\n<truncation_notice>\nFull output written to: /tmp/of/content.txt\n</truncation_notice>";
    const d = toolSummary("execute", undefined, content);
    const notes = d.segments.filter((s) => s.kind === "note").map((s) => s.text);
    expect(notes).toContain("… 6 earlier lines truncated");
    expect(notes).toContain("Full output: /tmp/of/content.txt");
    expect(notes.some((n) => n.includes("was parsed out"))).toBe(true);
    const code = d.segments.find((s) => s.kind === "code");
    expect(code?.kind === "code" && code.text).toBe("c8b6e5d..4b592e3  project -> project");
  });

  it("labels get_output as shell output", () => {
    const d = toolSummary("get_output", undefined, "aria-expanded before: true\n\n\nExit code: 0");
    expect(d.label).toBe("Read shell output");
  });

  it("supports cline-native arg arrays", () => {
    const d = toolSummary("run_commands", '{"commands":["ls","pwd"]}', "");
    expect(d.detail).toBe("ls");
    expect(d.segments[0]).toEqual({ kind: "command", text: "ls\npwd" });
  });

  it("decodes a JSON-string live result", () => {
    const d = toolSummary("Ran command", '{"command":"ls"}', JSON.stringify("file1\nfile2"));
    const code = d.segments.find((s) => s.kind === "code");
    expect(code?.kind === "code" && code.text).toBe("file1\nfile2");
  });

  it("IR exitCode overrides the parsed one and dedupes to a single note", () => {
    const content = "Output from command in shell x:\nok\n\n\nExit code: 0";
    const d = toolSummary("execute", undefined, content, { exitCode: 1 });
    const exits = d.segments.filter((s) => s.kind === "note" && s.text.startsWith("exit"));
    expect(exits).toHaveLength(1);
    expect(exits[0]?.kind === "note" && exits[0].text).toBe("exit 1");
    expect(exits[0]?.kind === "note" && exits[0].error).toBe(true);
  });

  it("IR exitCode lands a note even when the content carries none", () => {
    const d = toolSummary("execute", undefined, "some output", { exitCode: 0 });
    const exit = d.segments.find((s) => s.kind === "note" && s.text === "exit 0");
    expect(exit).toBeDefined();
    expect(exit?.kind === "note" && exit.error).toBeUndefined();
  });

  it("applies IR exitCode to non-exec tools too", () => {
    const d = toolSummary("custom_mcp", undefined, "failed silently", { exitCode: 2 });
    const exit = d.segments.find((s) => s.kind === "note" && s.text === "exit 2");
    expect(exit?.kind === "note" && exit.error).toBe(true);
  });

  it("absent or non-finite meta leaves the parsed note alone", () => {
    const content = "Output from command in shell x:\nok\n\n\nExit code: 3";
    const d = toolSummary("execute", undefined, content, { exitCode: Number.NaN });
    const exit = d.segments.find((s) => s.kind === "note" && s.text === "exit 3");
    expect(exit?.kind === "note" && exit.error).toBe(true);
  });
});

describe("toolSummary — read", () => {
  const fileView =
    '<file-view path="/repo/src/normalize.ts" start_line="1" end_line="3" total_lines="9">\n' +
    '  1|import type { X } from "./x";\n  2|\n  3|export const f = 1;\n';

  it("parses the file-view envelope into a path detail + code", () => {
    const d = toolSummary("read", undefined, fileView);
    expect(d.label).toBe("Read file");
    expect(d.detail).toBe("/repo/src/normalize.ts:1–3");
    expect(d.segments).toHaveLength(1);
    const seg = d.segments[0];
    expect(seg?.kind).toBe("code");
    expect(seg?.kind === "code" && seg.text).toBe(
      'import type { X } from "./x";\n\nexport const f = 1;',
    );
  });

  it("falls back to arg path when the result is not a file-view", () => {
    const d = toolSummary("Read file", '{"file_path":"/a/b.ts","offset":10,"limit":5}', "");
    expect(d.detail).toBe("/a/b.ts:10–14");
    expect(d.segments).toHaveLength(0);
  });

  it("supports cline-native files args", () => {
    const d = toolSummary("read_files", '{"files":[{"path":"/a/b.ts"}]}', "");
    expect(d.detail).toBe("/a/b.ts");
  });
});

describe("toolSummary — edit/write", () => {
  it("parses the 'has been updated' envelope", () => {
    const content =
      "The file /repo/a.ts has been updated. Here's the result of running `cat -n` on a snippet of the edited file:\n 89|  const x = 1;\n 90|});\n";
    const d = toolSummary("edit", undefined, content);
    expect(d.label).toBe("Edited file");
    expect(d.detail).toBe("/repo/a.ts");
    const seg = d.segments[0];
    // The `NN|` prefix goes; the content's own indentation stays.
    expect(seg?.kind === "code" && seg.text).toBe("  const x = 1;\n});");
  });

  it("parses 'File created successfully' as a write", () => {
    const d = toolSummary("write", undefined, "File created successfully at: /repo/new.ts");
    expect(d.label).toBe("Wrote file");
    expect(d.detail).toBe("/repo/new.ts");
  });

  it("builds a diff-ish segment from live edit args", () => {
    const d = toolSummary(
      "Edited file",
      '{"file_path":"/a.ts","old_string":"const x = 1;","new_string":"const x = 2;"}',
      "",
    );
    const seg = d.segments.find((s) => s.kind === "diff");
    expect(seg?.kind === "diff" && seg.text).toBe("- const x = 1;\n+ const x = 2;");
  });

  it("shows new file content from write args", () => {
    const d = toolSummary(
      "Wrote file",
      '{"file_path":"/a.ts","content":"line one\\nline two"}',
      "",
    );
    const seg = d.segments.find((s) => s.kind === "diff");
    expect(seg?.kind === "diff" && seg.text).toBe("+ line one\n+ line two");
  });
});

describe("toolSummary — search", () => {
  it("parses the 'Found N match(es)' envelope", () => {
    const content =
      "Found 18 match(es) for pattern 'foo|bar' in /repo:\n-- 2 matches in /repo/a.ts\n  1|foo\n  2|bar";
    const d = toolSummary("grep", undefined, content);
    expect(d.label).toBe("Searched codebase");
    expect(d.detail).toBe("foo|bar");
    const note = d.segments[0];
    expect(note?.kind === "note" && note.text).toBe("18 match(es) in /repo");
    const code = d.segments[1];
    expect(code?.kind === "code" && code.text).toContain("-- 2 matches in /repo/a.ts");
  });

  it("uses arg pattern for detail on live rows", () => {
    const d = toolSummary("Searched codebase", '{"pattern":"needle","path":"/repo"}', "");
    expect(d.detail).toBe("needle");
  });
});

describe("toolSummary — fetch + generic", () => {
  it("keeps fetch results as markdown with the query as detail", () => {
    const content =
      '# Web Search Results for "better-auth api key"\n\n## 1. API Key\nURL: https://x';
    const d = toolSummary("webfetch", undefined, content);
    expect(d.label).toBe("Fetched web content");
    expect(d.detail).toBe("better-auth api key");
    expect(d.segments[0]?.kind).toBe("markdown");
  });

  it("pretty-prints a JSON object result for unknown tools", () => {
    const d = toolSummary("custom_mcp", undefined, '{"key":"v"}');
    expect(d.label).toBe("Custom mcp");
    const seg = d.segments[0];
    expect(seg?.kind === "code" && seg.text).toBe('{\n  "key": "v"\n}');
  });

  it("renders plain text results as markdown for unknown tools", () => {
    const d = toolSummary("other", undefined, "Background subagent started with agent_id=abc.");
    expect(d.label).toBe("Tool call");
    expect(d.segments[0]?.kind).toBe("markdown");
  });

  it("renders todo lists as markdown", () => {
    const d = toolSummary(
      "todo_write",
      undefined,
      "Todos have been modified successfully.\nCurrent todo list:\n1. [x] done",
    );
    expect(d.segments[0]?.kind).toBe("markdown");
  });
});

describe("fileDiffView", () => {
  it("elides common prefix/suffix into unchanged markers with context", () => {
    const old = ["a", "b", "c", "same", "x = 1;", "tail1", "tail2", "tail3", "tail4", "tail5"];
    const neu = ["a", "b", "c", "same", "x = 2;", "tail1", "tail2", "tail3", "tail4", "tail5"];
    const v = fileDiffView({ path: "/a.ts", oldText: old.join("\n"), newText: neu.join("\n") });
    expect(v.added).toBe(1);
    expect(v.removed).toBe(1);
    expect(v.text).toBe(
      [
        "⋮ 4 unchanged lines",
        "  b",
        "  c",
        "  same",
        "- x = 1;",
        "+ x = 2;",
        "  tail1",
        "  tail2",
        "  tail3",
        "⋮ 5 unchanged lines",
      ].join("\n"),
    );
  });

  it("keeps an editor hunk (no shared context) whole", () => {
    const v = fileDiffView({ path: "/b.ts", oldText: "old line", newText: "new line" });
    expect(v.text).toBe("- old line\n+ new line");
    expect(v.added).toBe(1);
    expect(v.removed).toBe(1);
  });

  it("marks a create (no oldText) and an empty diff", () => {
    const created = fileDiffView({ path: "/n.ts", newText: "one\ntwo" });
    expect(created.text).toBe("+ one\n+ two");
    expect(created.removed).toBe(0);

    const empty = fileDiffView({ path: "/e.ts" });
    expect(empty.text).toBe("");
    expect(empty.added).toBe(0);
    expect(empty.removed).toBe(0);
  });

  it("produces diff-prefixed lines a `diff` grammar can highlight", () => {
    const v = fileDiffView({ path: "/b.ts", oldText: "old line", newText: "new line" });
    // Every line carries a diff marker: -/+/context (or the ⋮ elision note).
    for (const line of v.text.split("\n")) {
      expect(/^[-+ ⋮]/.test(line)).toBe(true);
    }
  });
});

describe("diffFence", () => {
  it("wraps text in a `diff` code fence", () => {
    expect(diffFence("- a\n+ b")).toBe("```diff\n- a\n+ b\n```");
  });

  it("widens the fence past backtick runs inside the payload", () => {
    const text = "+ const fence = ```diff```;";
    const fenced = diffFence(text);
    expect(fenced.startsWith("````diff\n")).toBe(true);
    expect(fenced.endsWith("\n````")).toBe(true);
  });
});

describe("toolContentSegments", () => {
  it("renders a terminal ref as a muted note, plus its inline output", () => {
    expect(
      toolContentSegments([
        { type: "terminal", terminalId: "term-1" },
        { type: "terminal", terminalId: "term-2", output: "build ok" },
      ]),
    ).toEqual([
      { kind: "note", text: "Terminal term-1" },
      { kind: "note", text: "Terminal term-2" },
      { kind: "code", text: "build ok" },
    ]);
  });

  it("renders embedded text blocks as code and images as markdown", () => {
    expect(
      toolContentSegments([
        { type: "text", text: "result text" },
        { type: "text", text: "   " },
        { type: "image", uri: "file:///shot.png" },
        { type: "image", data: "aGk=", mimeType: "image/png" },
      ]),
    ).toEqual([
      { kind: "code", text: "result text" },
      { kind: "markdown", text: "![tool output](file:///shot.png)" },
      { kind: "markdown", text: "![tool output](data:image/png;base64,aGk=)" },
    ]);
  });

  it("is empty when the call carries no contents", () => {
    expect(toolContentSegments(undefined)).toEqual([]);
    expect(toolContentSegments([])).toEqual([]);
  });
});

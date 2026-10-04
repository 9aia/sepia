import { describe, expect, it } from "vite-plus/test";
import { splitLeadingJson, stripAnsi, toolSummary } from "../lib/toolDisplay";

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

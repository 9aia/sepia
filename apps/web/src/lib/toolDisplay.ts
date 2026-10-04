/**
 * Pretty-rendering for tool rows. Two payload shapes arrive:
 *
 * - History (IR `tool` nodes): `content` is the agent's result *text* —
 *   Devin-style envelopes like `<file-view path=…>`, `Output from command in
 *   shell X:`, `The file P has been updated…`, `Found N match(es)…`. The
 *   call's args arrive as `argsJson` too (the IR `ToolCall.arguments`,
 *   JSON-encoded); older rows lack them, so the salient bits
 *   (path/command/query) are still parsed back out of the text as well.
 * - Live (AG-UI events): `argsJson` is the concatenated `rawInput` JSON
 *   (repeated snapshots while args stream), `content` the `rawOutput` JSON.
 *
 * Everything is heuristic: unrecognized names or payloads fall back to a
 * generic label + markdown body so nothing renders worse than before.
 */

export type ToolCategory = "exec" | "edit" | "read" | "search" | "fetch" | "todo" | "other";

export type ToolSegment =
  /** A `$ command` line. */
  | { readonly kind: "command"; readonly text: string }
  /** Monospace block — file snippets, shell output, JSON. */
  | { readonly kind: "code"; readonly text: string }
  /** old/new lines with -/+ prefixes, colored by the renderer. */
  | { readonly kind: "diff"; readonly text: string }
  /** Free text rendered through the markdown pipeline. */
  | { readonly kind: "markdown"; readonly text: string }
  /** Small status line — exit code, truncation notes. */
  | { readonly kind: "note"; readonly text: string; readonly error?: boolean };

export interface ToolDisplay {
  readonly category: ToolCategory;
  /** Human label for the marker line ("Read file", "Ran command"). */
  readonly label: string;
  /** The salient argument — path, command, or query. Mono, truncated. */
  readonly detail?: string;
  readonly segments: ReadonlyArray<ToolSegment>;
}

const ANSI_RE =
  // eslint-disable-next-line no-control-regex
  /\x1b(?:\[[0-9;?]*[a-zA-Z]|\][^\x07\x1b]*(?:\x07|\x1b\\)|[()][0-9A-B]|[=>])/g;

/** VT100/xterm escape sequences — exec results carry raw terminal output. */
export const stripAnsi = (text: string): string => text.replace(ANSI_RE, "");

/** Exclusive end index of the JSON value starting at `start`, or -1. */
const scanJsonValue = (text: string, start: number): number => {
  const first = text[start];
  if (first === '"') {
    let i = start + 1;
    while (i < text.length) {
      const c = text[i];
      if (c === "\\") {
        i += 2;
        continue;
      }
      if (c === '"') return i + 1;
      i += 1;
    }
    return -1;
  }
  if (first !== "{" && first !== "[") return -1;
  let depth = 0;
  let inString = false;
  for (let i = start; i < text.length; i++) {
    const c = text[i];
    if (inString) {
      if (c === "\\") {
        i += 1;
        continue;
      }
      if (c === '"') inString = false;
    } else if (c === '"') {
      inString = true;
    } else if (c === "{" || c === "[") {
      depth += 1;
    } else if (c === "}" || c === "]") {
      depth -= 1;
      if (depth === 0) return i + 1;
    }
  }
  return -1;
};

/**
 * Parses a run of concatenated JSON values (live args/results arrive as
 * `JSON.stringify` snapshots appended back-to-back). Returns the decoded
 * values plus whatever non-JSON text trails them — a partial snapshot while
 * still streaming lands in `rest`.
 */
export const splitLeadingJson = (
  text: string,
): { readonly values: ReadonlyArray<unknown>; readonly rest: string } => {
  const values: unknown[] = [];
  let i = 0;
  for (;;) {
    while (i < text.length && /\s/.test(text[i] as string)) i += 1;
    const c = text[i];
    if (c !== "{" && c !== "[" && c !== '"') break;
    const end = scanJsonValue(text, i);
    if (end === -1) break;
    try {
      values.push(JSON.parse(text.slice(i, end)));
    } catch {
      break;
    }
    i = end;
  }
  return { values, rest: text.slice(i) };
};

/** Last complete JSON object in an args stream, if any. */
const parseArgs = (raw: string | undefined): Record<string, unknown> | null => {
  if (raw === undefined) return null;
  const { values } = splitLeadingJson(raw);
  for (let i = values.length - 1; i >= 0; i--) {
    const v = values[i];
    if (typeof v === "object" && v !== null && !Array.isArray(v)) {
      return v as Record<string, unknown>;
    }
  }
  return null;
};

const TEXT_FIELDS = ["content", "text", "output", "result", "stdout", "message"] as const;

/** Pull a displayable string out of a result payload. */
const resultToText = (value: unknown): string => {
  if (typeof value === "string") return value;
  if (typeof value === "object" && value !== null) {
    const record = value as Record<string, unknown>;
    for (const field of TEXT_FIELDS) {
      const v = record[field];
      if (typeof v === "string" && v !== "") return v;
    }
  }
  return JSON.stringify(value, null, 2);
};

/**
 * Result payloads are usually plain text; live ones are one JSON value
 * (string/object) — decode it, keep any trailing non-JSON text.
 */
const resultText = (content: string): { readonly text: string; readonly raw: unknown } => {
  const trimmed = content.trim();
  if (trimmed === "") return { text: "", raw: undefined };
  const first = trimmed[0];
  if (first === "{" || first === "[" || first === '"') {
    const { values, rest } = splitLeadingJson(trimmed);
    if (values.length > 0) {
      const raw = values[values.length - 1];
      const text = resultToText(raw);
      const tail = rest.trim();
      return { text: tail === "" ? text : `${text}\n${rest}`, raw };
    }
  }
  return { text: content, raw: undefined };
};

const categorize = (toolName: string): ToolCategory => {
  const n = toolName.toLowerCase().replace(/[^a-z0-9]+/g, "");
  if (/exec|bash|shell|terminal|command|getoutput|runc|cmd|powershell/.test(n)) return "exec";
  if (/edit|write|wrote|creat|patch|insert|delete|rename|move/.test(n)) return "edit";
  if (/grep|glob|search|find|query|list|look/.test(n)) return "search";
  if (/fetch|browse|web|http|url|download/.test(n)) return "fetch";
  if (/todo|task|plan|checklist/.test(n)) return "todo";
  if (/read|view|open|cat|load|inspect/.test(n)) return "read";
  return "other";
};

/** "todo_write" → "Todo write"; "other"/"" → "Tool call". */
const prettifyName = (name: string): string => {
  const spaced = name
    .replace(/[_\-./]+/g, " ")
    .replace(/([a-z])([A-Z])/g, "$1 $2")
    .trim();
  if (spaced === "" || spaced.toLowerCase() === "other") return "Tool call";
  return spaced.charAt(0).toUpperCase() + spaced.slice(1);
};

const str = (v: unknown): string | undefined => (typeof v === "string" && v !== "" ? v : undefined);

/** First string of an array arg, e.g. cline's `{files: [{path}]}` / `{queries: [...]}`. */
const firstString = (v: unknown): string | undefined => {
  if (!Array.isArray(v)) return undefined;
  const first = v[0];
  if (typeof first === "string" && first !== "") return first;
  if (typeof first === "object" && first !== null) {
    const record = first as Record<string, unknown>;
    return str(record["path"]) ?? str(record["url"]);
  }
  return undefined;
};

/** `path` under common arg names (`file_path` devin/cline, `path`, `files[]`). */
const argPath = (args: Record<string, unknown> | null): string | undefined =>
  args === null
    ? undefined
    : (str(args["file_path"]) ??
      str(args["path"]) ??
      str(args["file"]) ??
      firstString(args["files"]));

/** Args wrapped one level deep — `{input: {command: …}}` and friends. */
const nestedCommand = (v: unknown): string | undefined => {
  if (typeof v !== "object" || v === null || Array.isArray(v)) return undefined;
  const record = v as Record<string, unknown>;
  return str(record["command"]) ?? str(record["cmd"]) ?? str(record["shell"]);
};

const argCommand = (args: Record<string, unknown> | null): string | undefined =>
  args === null
    ? undefined
    : (str(args["command"]) ??
      str(args["cmd"]) ??
      str(args["script"]) ??
      // `shell` reads as a command only when none of the unambiguous fields
      // set it — it can also name the shell binary (`shell: "bash"`).
      str(args["shell"]) ??
      nestedCommand(args["input"]) ??
      nestedCommand(args["params"]) ??
      (Array.isArray(args["commands"])
        ? (args["commands"] as unknown[])
            .filter((c): c is string => typeof c === "string")
            .join("\n") || undefined
        : undefined));

/** The shell id a `get_output`/`kill_shell`-style call targets. */
const argShellId = (args: Record<string, unknown> | null): string | undefined =>
  args === null ? undefined : (str(args["shell_id"]) ?? str(args["shellId"]));

const argQuery = (args: Record<string, unknown> | null): string | undefined =>
  args === null
    ? undefined
    : (str(args["pattern"]) ??
      str(args["query"]) ??
      str(args["regex"]) ??
      firstString(args["queries"]));

const argUrl = (args: Record<string, unknown> | null): string | undefined =>
  args === null
    ? undefined
    : (str(args["url"]) ??
      str(args["query"]) ??
      str(args["request"]) ??
      firstString(args["requests"]));

const argRange = (args: Record<string, unknown>): string => {
  const start = args["start_line"] ?? args["offset"];
  const end =
    args["end_line"] ??
    (typeof start === "number" && typeof args["limit"] === "number"
      ? (start as number) + (args["limit"] as number) - 1
      : undefined);
  return typeof start === "number" && typeof end === "number" ? `:${start}–${end}` : "";
};

/** Cap display size — history results can be thousands of lines. */
const capLines = (text: string, max: number): string => {
  const lines = text.split("\n");
  if (lines.length <= max) return text;
  return `${lines.slice(0, max).join("\n")}\n… (${lines.length - max} more lines)`;
};

/** Drop `NN|` line-number prefixes — content keeps its own indentation. */
const stripLineNumbers = (text: string): string => text.replace(/^\s*\d{1,6}\|/gm, "");

// -- result-text envelopes seen in stored sessions (devin + cline imports) --

const FILE_VIEW_RE = /^<file-view\s+([^>]*)>\s*\n?([\s\S]*)$/;
const FILE_VIEW_PATH_RE = /path="([^"]*)"/;
const FILE_VIEW_START_RE = /start_line="(\d+)"/;
const FILE_VIEW_END_RE = /end_line="(\d+)"/;

const EDIT_UPDATED_RE = /^The file (\S+) has been updated\.[\s\S]*?edited file:\n([\s\S]*)$/;
const CREATED_RE = /^File created successfully at:\s*(\S+)/;

const EXEC_HEADER_RE = /^Output from command in shell [^\n:]*:\n?/;
const EXIT_CODE_RE = /\n{0,3}Exit code: (-?\d+)\s*$/;
const TRUNC_LINES_RE = /^… \((\d+) lines? truncated\)\n?/;
const TRUNC_NOTICE_RE =
  /<truncation_notice>\s*Full output written to:\s*(\S+)\s*<\/truncation_notice>/;
const PARSED_OUT_RE = /\n*`[^`]*` was parsed out \(\d+ of \d+ (?:total )?lines shown\)\.\s*$/;

const SEARCH_RE = /^Found (\d+) match\(es\) for pattern '([^']*)' in (.+?):\n([\s\S]*)$/;
const FETCH_QUERY_RE = /^#\s*Web Search Results for "([^"]*)"/;

interface ExecParsed {
  readonly output: string;
  readonly notes: ReadonlyArray<ToolSegment>;
}

const parseExecResult = (text: string): ExecParsed => {
  const notes: ToolSegment[] = [];
  let body = stripAnsi(text).replace(EXEC_HEADER_RE, "");

  const trunc = TRUNC_NOTICE_RE.exec(body);
  if (trunc !== null) {
    notes.push({ kind: "note", text: `Full output: ${trunc[1]}` });
    body = body.replace(TRUNC_NOTICE_RE, "");
  }
  body = body.replace(PARSED_OUT_RE, (whole) => {
    notes.push({ kind: "note", text: whole.trim().replaceAll("`", "") });
    return "";
  });

  let exitCode: number | null = null;
  const exit = EXIT_CODE_RE.exec(body);
  if (exit !== null) {
    exitCode = Number(exit[1]);
    body = body.replace(EXIT_CODE_RE, "");
  }

  const truncated = TRUNC_LINES_RE.exec(body.trimStart());
  if (truncated !== null) {
    notes.unshift({ kind: "note", text: `… ${truncated[1]} earlier lines truncated` });
    body = body.trimStart().replace(TRUNC_LINES_RE, "");
  }

  if (exitCode !== null) {
    notes.push({
      kind: "note",
      text: exitCode === 0 ? "exit 0" : `exit ${exitCode}`,
      ...(exitCode === 0 ? {} : { error: true }),
    });
  }

  return { output: body.trim(), notes };
};

const buildExec = (
  toolName: string,
  args: Record<string, unknown> | null,
  result: { text: string; raw: unknown },
): ToolDisplay => {
  const command = argCommand(args);
  const rawRecord =
    typeof result.raw === "object" && result.raw !== null
      ? (result.raw as Record<string, unknown>)
      : null;
  const rawExit =
    rawRecord !== null
      ? [rawRecord["exitCode"], rawRecord["exit_code"], rawRecord["exit"], rawRecord["code"]].find(
          (v): v is number => typeof v === "number",
        )
      : undefined;

  const parsed = parseExecResult(result.text);
  const segments: ToolSegment[] = [];
  if (command !== undefined) segments.push({ kind: "command", text: command });
  if (parsed.output !== "") segments.push({ kind: "code", text: capLines(parsed.output, 200) });
  segments.push(...parsed.notes);
  if (rawExit !== undefined && !parsed.notes.some((n) => n.text.startsWith("exit "))) {
    segments.push({
      kind: "note",
      text: `exit ${rawExit}`,
      ...(rawExit === 0 ? {} : { error: true }),
    });
  }

  const shellId = argShellId(args);
  const label = /get_?output/i.test(toolName)
    ? "Read shell output"
    : /kill_?shell/i.test(toolName)
      ? "Killed shell"
      : "Ran command";
  // A call that only carries a shell id (get_output, kill_shell) still gets
  // a detail — it just isn't a `$ command` line.
  const detail =
    command?.split("\n")[0] ?? (shellId !== undefined ? `shell ${shellId}` : undefined);
  return {
    category: "exec",
    label,
    ...(detail !== undefined ? { detail } : {}),
    segments,
  };
};

const buildRead = (
  args: Record<string, unknown> | null,
  result: { text: string; raw: unknown },
): ToolDisplay => {
  const view = FILE_VIEW_RE.exec(result.text.trim());
  const argDetail = argPath(args);
  if (view === null) {
    // Live row before the result lands, or an unrecognized read payload.
    const segments: ToolSegment[] = [];
    if (result.text.trim() !== "") {
      segments.push({ kind: "code", text: capLines(result.text.trim(), 200) });
    }
    return {
      category: "read",
      label: "Read file",
      ...(argDetail !== undefined
        ? { detail: `${argDetail}${args !== null ? argRange(args) : ""}` }
        : {}),
      segments,
    };
  }
  const attrs = view[1] as string;
  const path = FILE_VIEW_PATH_RE.exec(attrs)?.[1];
  const start = FILE_VIEW_START_RE.exec(attrs)?.[1];
  const end = FILE_VIEW_END_RE.exec(attrs)?.[1];
  const detail =
    path !== undefined
      ? `${path}${start !== undefined && end !== undefined ? `:${start}–${end}` : ""}`
      : argDetail;
  const code = stripLineNumbers(view[2] as string).replace(/\n+$/, "");
  return {
    category: "read",
    label: "Read file",
    ...(detail !== undefined ? { detail } : {}),
    segments: code === "" ? [] : [{ kind: "code", text: capLines(code, 300) }],
  };
};

const buildDiffText = (oldText: string, newText: string): string => {
  const cap = (t: string) => capLines(t, 60);
  const lines: string[] = [];
  if (oldText !== "")
    lines.push(
      ...cap(oldText)
        .split("\n")
        .map((l) => `- ${l}`),
    );
  if (newText !== "")
    lines.push(
      ...cap(newText)
        .split("\n")
        .map((l) => `+ ${l}`),
    );
  return lines.join("\n");
};

/**
 * One file's recorded before/after rendered for the tool row: common
 * leading/trailing lines elide to `⋮` markers (with a few lines of context)
 * so a whole-file `oldText`/`newText` pair still reads as the hunk it was.
 * `added`/`removed` count the lines the elision kept, i.e. the region that
 * actually changed — honest stats, not a full Myers diff.
 */
export interface FileDiffView {
  readonly path: string;
  readonly text: string;
  readonly added: number;
  readonly removed: number;
}

const DIFF_CONTEXT = 3;
const DIFF_MAX_MIDDLE = 120;

export const fileDiffView = (diff: {
  readonly path: string;
  readonly oldText?: string;
  readonly newText?: string;
}): FileDiffView => {
  const oldLines = diff.oldText === undefined ? [] : diff.oldText.split("\n");
  const newLines = diff.newText === undefined ? [] : diff.newText.split("\n");

  let pre = 0;
  const maxPre = Math.min(oldLines.length, newLines.length);
  while (pre < maxPre && oldLines[pre] === newLines[pre]) pre += 1;
  let suf = 0;
  const maxSuf = maxPre - pre;
  while (
    suf < maxSuf &&
    oldLines[oldLines.length - 1 - suf] === newLines[newLines.length - 1 - suf]
  ) {
    suf += 1;
  }

  let oldMid = oldLines.slice(pre, oldLines.length - suf);
  let newMid = newLines.slice(pre, newLines.length - suf);
  const removed = oldMid.length;
  const added = newMid.length;
  let clipped = 0;
  if (oldMid.length > DIFF_MAX_MIDDLE) {
    clipped += oldMid.length - DIFF_MAX_MIDDLE;
    oldMid = oldMid.slice(0, DIFF_MAX_MIDDLE);
  }
  if (newMid.length > DIFF_MAX_MIDDLE) {
    clipped += newMid.length - DIFF_MAX_MIDDLE;
    newMid = newMid.slice(0, DIFF_MAX_MIDDLE);
  }

  const out: string[] = [];
  if (pre > 0) {
    out.push(`⋮ ${pre} unchanged line${pre === 1 ? "" : "s"}`);
    for (const line of oldLines.slice(Math.max(0, pre - DIFF_CONTEXT), pre)) out.push(`  ${line}`);
  }
  out.push(...oldMid.map((l) => `- ${l}`));
  out.push(...newMid.map((l) => `+ ${l}`));
  if (clipped > 0) out.push(`⋮ ${clipped} more changed lines`);
  if (suf > 0) {
    for (const line of oldLines.slice(
      oldLines.length - suf,
      oldLines.length - suf + DIFF_CONTEXT,
    )) {
      out.push(`  ${line}`);
    }
    out.push(`⋮ ${suf} unchanged line${suf === 1 ? "" : "s"}`);
  }
  return { path: diff.path, text: out.join("\n"), added, removed };
};

/**
 * A markdown code fence carrying `text` as a `diff` block — streamdown/Shiki
 * then renders it with +/- coloring inside the shared code-block chrome.
 * The fence widens past any backtick run in the payload so source text that
 * itself contains ``` can't break out of the block.
 */
export const diffFence = (text: string): string => {
  const longest = (text.match(/`{3,}/g) ?? []).reduce((n, run) => Math.max(n, run.length), 0);
  const fence = "`".repeat(Math.max(3, longest + 1));
  return `${fence}diff\n${text}\n${fence}`;
};

const buildEdit = (
  toolName: string,
  args: Record<string, unknown> | null,
  result: { text: string; raw: unknown },
): ToolDisplay => {
  const text = result.text.trim();
  const updated = EDIT_UPDATED_RE.exec(text);
  const created = CREATED_RE.exec(text);
  const path = updated?.[1] ?? created?.[1] ?? argPath(args);
  const isWrite =
    created !== null || /write|wrote|creat/.test(toolName.toLowerCase().replace(/[^a-z]+/g, ""));
  const label = isWrite ? "Wrote file" : "Edited file";

  const segments: ToolSegment[] = [];
  if (updated !== null) {
    const snippet = stripLineNumbers(updated[2] as string).replace(/\n+$/, "");
    if (snippet !== "") segments.push({ kind: "code", text: capLines(snippet, 200) });
  } else if (created !== null) {
    segments.push({ kind: "note", text: "File created" });
  } else if (args !== null) {
    const oldText = str(args["old_string"]) ?? str(args["old_text"]);
    const newText = str(args["new_string"]) ?? str(args["new_text"]) ?? str(args["content"]);
    if (oldText !== undefined || newText !== undefined) {
      segments.push({ kind: "diff", text: buildDiffText(oldText ?? "", newText ?? "") });
    }
  }
  if (segments.length === 0 && text !== "") {
    segments.push({ kind: "markdown", text: capLines(text, 120) });
  }
  return {
    category: "edit",
    label,
    ...(path !== undefined ? { detail: path } : {}),
    segments,
  };
};

const buildSearch = (
  args: Record<string, unknown> | null,
  result: { text: string; raw: unknown },
): ToolDisplay => {
  const text = result.text.trim();
  const found = SEARCH_RE.exec(text);
  const query = argQuery(args);
  if (found === null) {
    return {
      category: "search",
      label: "Searched codebase",
      ...(query !== undefined ? { detail: query } : {}),
      segments: text === "" ? [] : [{ kind: "code", text: capLines(text, 200) }],
    };
  }
  return {
    category: "search",
    label: "Searched codebase",
    detail: found[2] as string,
    segments: [
      { kind: "note", text: `${found[1]} match(es) in ${found[3]}` },
      { kind: "code", text: capLines((found[4] as string).trim(), 200) },
    ],
  };
};

const buildFetch = (
  args: Record<string, unknown> | null,
  result: { text: string; raw: unknown },
): ToolDisplay => {
  const text = result.text.trim();
  const query = argUrl(args) ?? FETCH_QUERY_RE.exec(text)?.[1];
  return {
    category: "fetch",
    label: "Fetched web content",
    ...(query !== undefined ? { detail: query } : {}),
    segments: text === "" ? [] : [{ kind: "markdown", text: capLines(text, 150) }],
  };
};

const buildGeneric = (toolName: string, result: { text: string; raw: unknown }): ToolDisplay => {
  const text = result.text.trim();
  if (result.raw !== undefined && typeof result.raw !== "string") {
    // Structured result we didn't pull text out of — show it pretty-printed.
    return {
      category: "other",
      label: prettifyName(toolName),
      segments: [{ kind: "code", text: capLines(JSON.stringify(result.raw, null, 2), 120) }],
    };
  }
  return {
    category: "other",
    label: prettifyName(toolName),
    segments: text === "" ? [] : [{ kind: "markdown", text: capLines(text, 150) }],
  };
};

/** IR v2 fields carried alongside the row — authoritative over parsed text. */
export interface ToolCallMeta {
  readonly exitCode?: number;
}

const EXIT_NOTE_RE = /^exit -?\d+$/;

const withExitCode = (display: ToolDisplay, exitCode: number): ToolDisplay => ({
  ...display,
  segments: [
    ...display.segments.filter((s) => !(s.kind === "note" && EXIT_NOTE_RE.test(s.text))),
    {
      kind: "note",
      text: `exit ${exitCode}`,
      ...(exitCode === 0 ? {} : { error: true }),
    },
  ],
});

/**
 * Maps a tool row to a label, a one-line detail, and typed body segments.
 * `argsJson` is the live arg stream (undefined for history rows); `content`
 * is the stored/live result text. `meta.exitCode` (IR v2) is authoritative —
 * it replaces any `exit N` note parsed out of the content.
 */
export const toolSummary = (
  toolName: string,
  argsJson: string | undefined,
  content: string,
  meta?: ToolCallMeta,
): ToolDisplay => {
  const args = parseArgs(argsJson);
  const result = resultText(content);
  let display: ToolDisplay;
  switch (categorize(toolName)) {
    case "exec":
      display = buildExec(toolName, args, result);
      break;
    case "read":
      display = buildRead(args, result);
      break;
    case "edit":
      display = buildEdit(toolName, args, result);
      break;
    case "search":
      display = buildSearch(args, result);
      break;
    case "fetch":
      display = buildFetch(args, result);
      break;
    case "todo":
      display = { ...buildGeneric(toolName, result), label: "Updated todos" };
      break;
    case "other":
      display = buildGeneric(toolName, result);
      break;
  }
  const exitCode = meta?.exitCode;
  if (exitCode === undefined || !Number.isFinite(exitCode)) return display;
  return withExitCode(display, exitCode);
};

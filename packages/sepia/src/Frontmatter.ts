/**
 * Minimal YAML-frontmatter reader/writer for agent config files — skills,
 * rules, commands, agents all ship as `---\n<yaml>\n---\n<body>` markdown.
 *
 * This is a deliberate subset, not a YAML implementation: it covers what the
 * four agents actually emit (scalar keys, `- item` lists, `|`/`>` block
 * scalars, one level of nested maps, `[a, b]` flow lists) and leaves anything
 * else untouched as a raw string so callers can pass it through `metadata`.
 */

export interface FrontmatterDoc {
  readonly attributes: Record<string, unknown>;
  readonly body: string;
}

/** True when `text` opens with a `---` fence on its own first line. */
export const has = (text: string): boolean => /^---[ \t]*\r?\n/.test(text);

const indentOf = (line: string): number => {
  const match = /^ */.exec(line);
  return match === null ? 0 : match[0].length;
};

const isBlank = (line: string): boolean => line.trim() === "";

const BLOCK_SCALAR = /^([>|])([+-])?$/;

const parseScalar = (raw: string): unknown => {
  const s = raw.trim();
  if (s === "") return "";
  if (s === "true") return true;
  if (s === "false") return false;
  if (s === "null" || s === "~") return null;
  if (/^-?\d+(\.\d+)?$/.test(s)) return Number(s);
  if (s.length >= 2 && s.startsWith('"') && s.endsWith('"')) {
    try {
      return JSON.parse(s) as unknown;
    } catch {
      return s.slice(1, -1);
    }
  }
  if (s.length >= 2 && s.startsWith("'") && s.endsWith("'")) {
    return s.slice(1, -1).replace(/''/g, "'");
  }
  if (s.startsWith("[") && s.endsWith("]")) {
    const inner = s.slice(1, -1).trim();
    if (inner === "") return [];
    return inner.split(",").map((part) => parseScalar(part));
  }
  return s;
};

interface ParseState {
  readonly lines: ReadonlyArray<string>;
}

/** Index of the next non-blank line at or after `i`, or lines.length. */
const nextContent = (st: ParseState, i: number): number => {
  let j = i;
  while (j < st.lines.length && isBlank(st.lines[j])) j += 1;
  return j;
};

/**
 * Collect lines strictly more indented than `indent` (or blank) — the body
 * of a `key:` with no inline value or a `|`/`>` block scalar.
 */
const collectChildren = (
  st: ParseState,
  i: number,
  indent: number,
): { readonly lines: ReadonlyArray<string>; readonly next: number } => {
  const out: string[] = [];
  let j = i;
  while (j < st.lines.length) {
    const line = st.lines[j];
    if (!isBlank(line) && indentOf(line) <= indent) break;
    out.push(line);
    j += 1;
  }
  // Trim leading/trailing blank lines inside the collected block.
  let start = 0;
  let end = out.length;
  while (start < end && isBlank(out[start])) start += 1;
  while (end > start && isBlank(out[end - 1])) end -= 1;
  return { lines: out.slice(start, end), next: j };
};

const dedent = (lines: ReadonlyArray<string>): ReadonlyArray<string> => {
  let min = Number.POSITIVE_INFINITY;
  for (const line of lines) {
    if (isBlank(line)) continue;
    min = Math.min(min, indentOf(line));
  }
  if (!Number.isFinite(min) || min === 0) return lines;
  return lines.map((line) => (isBlank(line) ? "" : line.slice(min)));
};

const parseNode = (st: ParseState, i: number, indent: number): { value: unknown; next: number } => {
  const first = nextContent(st, i);
  if (first >= st.lines.length) return { value: {}, next: first };
  const trimmed = st.lines[first].trim();
  if (trimmed.startsWith("- ") || trimmed === "-") {
    return parseList(st, first, indentOf(st.lines[first]));
  }
  return parseMap(st, first, indent);
};

const parseList = (st: ParseState, i: number, indent: number): { value: unknown; next: number } => {
  const items: unknown[] = [];
  let j = i;
  while (j < st.lines.length) {
    const line = st.lines[j];
    if (isBlank(line)) {
      j += 1;
      continue;
    }
    if (indentOf(line) !== indent || !line.trim().startsWith("-")) break;
    const afterDash = line.trim().slice(1).trim();
    if (afterDash === "") {
      // `-` alone: nested block on following deeper-indented lines.
      const child = collectChildren(st, j + 1, indent);
      if (child.lines.length > 0) {
        const inner = parseNode({ lines: dedent(child.lines) }, 0, 0);
        items.push(inner.value);
      } else {
        items.push(null);
      }
      j = child.next;
      continue;
    }
    items.push(parseScalar(afterDash));
    j += 1;
  }
  return { value: items, next: j };
};

const KEY_LINE = /^([^\s:#][^:]*):[ \t]*(.*)$/;

const parseMap = (st: ParseState, i: number, indent: number): { value: unknown; next: number } => {
  const map: Record<string, unknown> = {};
  let j = i;
  while (j < st.lines.length) {
    const line = st.lines[j];
    if (isBlank(line)) {
      j += 1;
      continue;
    }
    const lineIndent = indentOf(line);
    if (lineIndent !== indent) break;
    const match = KEY_LINE.exec(line);
    if (match === null) break;
    const key = match[1].trim();
    const rest = match[2];
    if (rest === "") {
      // Look ahead: deeper-indented content → child node; otherwise null.
      const probe = nextContent(st, j + 1);
      if (probe < st.lines.length && indentOf(st.lines[probe]) > indent) {
        const child = collectChildren(st, j + 1, indent);
        const inner = parseNode({ lines: dedent(child.lines) }, 0, 0);
        map[key] = inner.value;
        j = child.next;
      } else {
        map[key] = null;
        j += 1;
      }
      continue;
    }
    const blockMatch = BLOCK_SCALAR.exec(rest.trim());
    if (blockMatch !== null) {
      const child = collectChildren(st, j + 1, indent);
      const flat = dedent(child.lines);
      const folded = blockMatch[1] === ">";
      map[key] = folded
        ? flat
            .join(" ")
            .replace(/[ \t]+/g, " ")
            .trim()
        : flat.join("\n");
      j = child.next;
      continue;
    }
    map[key] = parseScalar(rest);
    j += 1;
  }
  return { value: map, next: j };
};

/**
 * Split `text` into frontmatter attributes and body. No `---` fence means
 * `{}` attributes and the whole text as body — rules files commonly carry
 * no frontmatter at all.
 */
export const parse = (text: string): FrontmatterDoc => {
  if (!has(text)) return { attributes: {}, body: text };
  const lines = text.split(/\r?\n/);
  // Find the closing fence (a `---` line after the opening one).
  let close = -1;
  for (let i = 1; i < lines.length; i += 1) {
    if (/^---[ \t]*$/.test(lines[i])) {
      close = i;
      break;
    }
    if (lines[i].trim() !== "" && indentOf(lines[i]) === 0 && !KEY_LINE.test(lines[i])) {
      // A non-key, non-indented line before the close means this isn't a
      // real frontmatter block (e.g. a markdown `---` hr); bail out.
      break;
    }
  }
  if (close === -1) return { attributes: {}, body: text };
  const yaml = lines.slice(1, close);
  const body = lines
    .slice(close + 1)
    .join("\n")
    .replace(/^\n+/, "");
  const st: ParseState = { lines: yaml };
  const parsed = parseNode(st, 0, 0).value;
  const attributes =
    parsed !== null && typeof parsed === "object" && !Array.isArray(parsed)
      ? (parsed as Record<string, unknown>)
      : {};
  return { attributes, body };
};

const NEEDS_QUOTE =
  /^\s|\s$|[:#'"[\]{}&*!|>@`%,\n]|^(true|false|null|~|yes|no|on|off|-?\d+(\.\d+)?)$/i;

/** YAML-safe scalar: plain when unambiguous, JSON-quoted otherwise. */
const scalarOut = (value: unknown): string => {
  if (value === null || value === undefined) return "null";
  if (typeof value === "boolean" || typeof value === "number") return String(value);
  if (typeof value === "object") return JSON.stringify(value);
  const s = String(value as string);
  if (s.includes("\n")) return JSON.stringify(s);
  return NEEDS_QUOTE.test(s) ? JSON.stringify(s) : s;
};

const renderEntry = (key: string, value: unknown, indent: string): ReadonlyArray<string> => {
  if (Array.isArray(value)) {
    if (value.length === 0) return [`${indent}${key}: []`];
    return [`${indent}${key}:`, ...value.map((item) => `${indent}  - ${scalarOut(item)}`)];
  }
  if (value !== null && typeof value === "object") {
    const nested = renderMap(value as Record<string, unknown>, `${indent}  `);
    return [`${indent}${key}:`, ...nested];
  }
  if (typeof value === "string" && value.includes("\n")) {
    return [`${indent}${key}: |`, ...value.split("\n").map((line) => `${indent}  ${line}`)];
  }
  return [`${indent}${key}: ${scalarOut(value)}`];
};

const renderMap = (attrs: Record<string, unknown>, indent: string): ReadonlyArray<string> =>
  Object.entries(attrs).flatMap(([key, value]) => renderEntry(key, value, indent));

/**
 * Rebuild the markdown file: a `---` fence when `attributes` is non-empty
 * (attribute order preserved), then the body verbatim. Empty attributes
 * produce the bare body — matching files that never had frontmatter.
 */
export const render = (attributes: Record<string, unknown>, body: string): string => {
  const entries = Object.entries(attributes).filter(([, v]) => v !== undefined);
  if (entries.length === 0) return body.endsWith("\n") ? body : `${body}\n`;
  const yaml = renderMap(Object.fromEntries(entries), "");
  const trimmedBody = body.replace(/^\n+/, "");
  return `---\n${yaml.join("\n")}\n---\n\n${trimmedBody}`;
};

import { expect, test } from "vite-plus/test";
import * as Frontmatter from "../src/Frontmatter.js";

test("parse returns the whole text as body when there is no fence", () => {
  const doc = Frontmatter.parse("# Title\n\nbody text\n");
  expect(doc.attributes).toEqual({});
  expect(doc.body).toBe("# Title\n\nbody text\n");
});

test("parse reads scalar attributes of every shape", () => {
  const doc = Frontmatter.parse(
    [
      "---",
      "name: my-skill",
      "description: a folded thing",
      "alwaysApply: true",
      "count: 3",
      "ratio: 1.5",
      "empty:",
      'quoted: "has: colon"',
      "single: 'it''s'",
      "---",
      "",
      "body here",
    ].join("\n"),
  );
  expect(doc.attributes).toEqual({
    name: "my-skill",
    description: "a folded thing",
    alwaysApply: true,
    count: 3,
    ratio: 1.5,
    empty: null,
    quoted: "has: colon",
    single: "it's",
  });
  expect(doc.body).toBe("body here");
});

test("parse handles folded (>) and literal (|) block scalars", () => {
  const doc = Frontmatter.parse(
    [
      "---",
      "description: >-",
      "  Create Cursor rules for",
      "  persistent AI guidance.",
      "note: |",
      "  line one",
      "  line two",
      "---",
      "body",
    ].join("\n"),
  );
  expect(doc.attributes["description"]).toBe("Create Cursor rules for persistent AI guidance.");
  expect(doc.attributes["note"]).toBe("line one\nline two");
});

test("parse handles dash lists, nested maps and flow lists", () => {
  const doc = Frontmatter.parse(
    [
      "---",
      "allowed-tools:",
      "  - read",
      "  - grep",
      "permissions:",
      "  allow:",
      "    - Exec(git)",
      "    - Read(**)",
      "globs: [*.ts, *.tsx]",
      "---",
      "x",
    ].join("\n"),
  );
  expect(doc.attributes["allowed-tools"]).toEqual(["read", "grep"]);
  expect(doc.attributes["permissions"]).toEqual({ allow: ["Exec(git)", "Read(**)"] });
  expect(doc.attributes["globs"]).toEqual(["*.ts", "*.tsx"]);
});

test("parse bails when the fence never closes or a non-key line interrupts", () => {
  const noClose = Frontmatter.parse("---\nname: x\nbody without close");
  expect(noClose.attributes).toEqual({});
  const hr = Frontmatter.parse("---\n\na thematic break\n---\nrest");
  expect(hr.attributes).toEqual({});
});

test("has() detects the opening fence", () => {
  expect(Frontmatter.has("---\nname: x\n---\n")).toBe(true);
  expect(Frontmatter.has("# no fence")).toBe(false);
});

test("render emits bare body for empty attributes", () => {
  expect(Frontmatter.render({}, "plain body")).toBe("plain body\n");
  expect(Frontmatter.render({}, "plain body\n")).toBe("plain body\n");
});

test("render round-trips through parse", () => {
  const attrs = {
    name: "skill",
    description: "uses a: colon",
    alwaysApply: true,
    globs: ["*.ts", "*.tsx"],
    permissions: { allow: ["Exec(git)"] },
    note: "line one\nline two",
  };
  const text = Frontmatter.render(attrs, "the body\n");
  const doc = Frontmatter.parse(text);
  expect(doc.attributes).toEqual(attrs);
  expect(doc.body).toBe("the body\n");
});

test("render quotes ambiguous scalars and keeps plain ones plain", () => {
  const text = Frontmatter.render(
    { plain: "hello-world", booly: "true", colon: "a: b", list: "a, b" },
    "x",
  );
  expect(text).toContain("plain: hello-world");
  expect(text).toContain('booly: "true"');
  expect(text).toContain('colon: "a: b"');
  const doc = Frontmatter.parse(text);
  expect(doc.attributes).toEqual({
    plain: "hello-world",
    booly: "true",
    colon: "a: b",
    list: "a, b",
  });
});

test("render emits null and object values losslessly enough to parse back", () => {
  const text = Frontmatter.render({ nothing: null, nested: { a: 1 } }, "x");
  const doc = Frontmatter.parse(text);
  expect(doc.attributes["nothing"]).toBeNull();
  expect(doc.attributes["nested"]).toEqual({ a: 1 });
});

test("parse handles bare-dash list items and scalar edge cases", () => {
  const doc = Frontmatter.parse(
    [
      "---",
      "items:",
      "  -",
      "  - one",
      "neg: -5",
      "empty-list: []",
      'badquote: "unterminated',
      "numstr: '007'",
      "---",
      "x",
    ].join("\n"),
  );
  expect(doc.attributes["items"]).toEqual([null, "one"]);
  expect(doc.attributes["neg"]).toBe(-5);
  expect(doc.attributes["empty-list"]).toEqual([]);
  // A lone unterminated quote stays literal — the subset parser keeps raw text.
  expect(doc.attributes["badquote"]).toBe('"unterminated');
  expect(doc.attributes["numstr"]).toBe("007");
});

test("parse bails on a bare list fence — no key line means no frontmatter", () => {
  const text = "---\n- a\n- b\n---\nbody";
  const doc = Frontmatter.parse(text);
  expect(doc.attributes).toEqual({});
  expect(doc.body).toBe(text);
});

test("parse treats a top-level map child with trailing blanks correctly", () => {
  const doc = Frontmatter.parse("---\nkey:\n  nested:\n    - x\nnext: v\n---\nb");
  expect(doc.attributes).toEqual({ key: { nested: ["x"] }, next: "v" });
});

test("render emits empty arrays and multiline literal scalars", () => {
  const text = Frontmatter.render({ empty: [], multi: "one\ntwo\nthree" }, "body");
  expect(text).toContain("empty: []");
  expect(text).toContain("multi: |");
  const doc = Frontmatter.parse(text);
  expect(doc.attributes["multi"]).toBe("one\ntwo\nthree");
  expect(doc.attributes["empty"]).toEqual([]);
});

test("render keeps object scalars inside lists as JSON", () => {
  const text = Frontmatter.render({ list: ["a", { k: 1 }] }, "x");
  const doc = Frontmatter.parse(text);
  // JSON-serialized objects read back as strings — a documented subset limit.
  expect(doc.attributes["list"]).toEqual(["a", '{"k":1}']);
});

test("parse unquotes a broken JSON string by slicing quotes", () => {
  const doc = Frontmatter.parse('---\nkey: "a"b"\n---\nx');
  expect(doc.attributes["key"]).toBe('a"b');
});

test("parse skips blank lines inside maps and lists", () => {
  const doc = Frontmatter.parse("---\na: 1\n\nb: 2\n\nitems:\n  - x\n\n  - y\n---\nz");
  expect(doc.attributes).toEqual({ a: 1, b: 2, items: ["x", "y"] });
});

test("parse reads a bare dash item with a nested block", () => {
  const doc = Frontmatter.parse("---\nitems:\n  -\n    - x\n    - y\n  - z\n---\nb");
  expect(doc.attributes["items"]).toEqual([["x", "y"], "z"]);
});

test("parse handles |+ chomping and an indented non-key line", () => {
  const doc = Frontmatter.parse("---\nnote: |+\n  kept\n  \nkey: v\n---\nb");
  // The subset parser treats `|`/`|-`/`|+` the same (trailing blanks trimmed).
  expect(doc.attributes["note"]).toBe("kept");
  // An indented stray line ends the top-level map early.
  const stray = Frontmatter.parse("---\n  orphan\nkey: v\n---\nb");
  expect(stray.attributes).toEqual({});
});

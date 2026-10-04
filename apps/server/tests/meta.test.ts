import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vite-plus/test";
import { appendSpan, createMetaStore } from "../src/meta";

const tempStore = () => {
  const dir = mkdtempSync(join(tmpdir(), "sepia-meta-"));
  const path = join(dir, "meta.json");
  return { dir, path, store: createMetaStore(path) };
};

describe("createMetaStore", () => {
  it("patches, reads, and removes session meta — persisted to disk", () => {
    const { path, store } = tempStore();
    store.patch("s1", { title: "Hello", pinned: true });
    expect(store.of("s1")).toMatchObject({ title: "Hello", pinned: true });
    expect(store.sessions()).toHaveProperty("s1");

    store.patch("s1", { archived: true });
    expect(store.of("s1")).toMatchObject({ title: "Hello", pinned: true, archived: true });

    // A second store over the same file sees the writes — they hit disk.
    const reloaded = createMetaStore(path);
    expect(reloaded.of("s1")).toMatchObject({ title: "Hello", archived: true });

    reloaded.remove("s1");
    expect(reloaded.of("s1")).toBeUndefined();
    reloaded.remove("s1"); // removing an unknown id is a no-op
    reloaded.remove("ghost");
  });

  it("manages the project lifecycle", () => {
    const { store } = tempStore();
    expect(store.listProjects()).toEqual([]);

    const project = store.createProject("Work");
    expect(project.id).toMatch(/^proj_[0-9a-f]{8}$/);
    expect(store.listProjects()).toEqual([{ id: project.id, name: "Work" }]);

    expect(store.renameProject(project.id, "Job")).toBe(true);
    expect(store.listProjects()[0]?.name).toBe("Job");
    expect(store.renameProject("proj_ghost", "X")).toBe(false);

    store.deleteProject(project.id);
    expect(store.listProjects()).toEqual([]);
    store.deleteProject("proj_ghost"); // no-op
  });

  it("deleting a project strips its id from session overlays", () => {
    const { store } = tempStore();
    const project = store.createProject("P");
    store.patch("s1", { projectIds: [project.id, "proj_other"] });
    store.patch("s2", { title: "untouched" });
    store.patch("s3", { projectIds: [project.id] });

    store.deleteProject(project.id);
    expect(store.of("s1")?.projectIds).toEqual(["proj_other"]);
    expect(store.of("s3")?.projectIds).toEqual([]);
    expect(store.of("s2")?.title).toBe("untouched");
  });

  it("stores arbitrary config keys", () => {
    const { path, store } = tempStore();
    store.setConfig("ui.section", { collapsed: true });
    expect(store.config()).toEqual({ "ui.section": { collapsed: true } });
    expect(createMetaStore(path).config()).toEqual({ "ui.section": { collapsed: true } });
  });

  it("loads a v2 file with sessions, projects, and config", () => {
    const dir = mkdtempSync(join(tmpdir(), "sepia-meta-"));
    const path = join(dir, "meta.json");
    writeFileSync(
      path,
      JSON.stringify({
        sessions: { s1: { title: "T", pinned: true, projectIds: ["p1"] } },
        projects: { p1: { name: "One" } },
        config: { theme: "dark" },
      }),
    );
    const store = createMetaStore(path);
    expect(store.of("s1")).toMatchObject({ title: "T", pinned: true, projectIds: ["p1"] });
    expect(store.listProjects()).toEqual([{ id: "p1", name: "One" }]);
    expect(store.config()).toEqual({ theme: "dark" });
  });

  it("migrates a v1 flat file: top-level session map, singular projectId", () => {
    const dir = mkdtempSync(join(tmpdir(), "sepia-meta-"));
    const path = join(dir, "meta.json");
    writeFileSync(
      path,
      JSON.stringify({
        s1: { title: "Old", projectId: "p9", pinned: "yes", model: 42 },
        s2: "not-an-object",
      }),
    );
    const store = createMetaStore(path);
    // projectId → projectIds; non-boolean flags normalize to false; a
    // non-string/null model drops to undefined.
    expect(store.of("s1")).toMatchObject({
      title: "Old",
      projectIds: ["p9"],
      pinned: false,
      model: undefined,
    });
    expect(store.of("s2")).toMatchObject({ title: undefined, projectIds: [] });
  });

  it("normalizes stored metas: non-string projectIds drop, model null is kept", () => {
    const dir = mkdtempSync(join(tmpdir(), "sepia-meta-"));
    const path = join(dir, "meta.json");
    writeFileSync(
      path,
      JSON.stringify({
        sessions: { s1: { projectIds: ["ok", 5, null], model: null, archived: true } },
      }),
    );
    const meta = createMetaStore(path).of("s1");
    expect(meta?.projectIds).toEqual(["ok"]);
    expect(meta?.model).toBeNull();
    expect(meta?.archived).toBe(true);
  });

  it("a corrupt file degrades to an empty store", () => {
    const dir = mkdtempSync(join(tmpdir(), "sepia-meta-"));
    const path = join(dir, "meta.json");
    writeFileSync(path, "{half-written");
    const store = createMetaStore(path);
    expect(store.sessions()).toEqual({});
    store.patch("s1", { title: "recovered" });
    expect(JSON.parse(readFileSync(path, "utf8"))).toHaveProperty("sessions.s1.title", "recovered");
  });

  it("non-object JSON degrades to an empty store", () => {
    const dir = mkdtempSync(join(tmpdir(), "sepia-meta-"));
    const path = join(dir, "meta.json");
    writeFileSync(path, JSON.stringify("just a string"));
    expect(createMetaStore(path).sessions()).toEqual({});
  });

  it("a top-level array reads as a v1 flat map keyed by index", () => {
    // Quirk of the v1 migration path: arrays are objects, so indices become
    // session ids with normalized (empty) metas. Harmless, but pinned down.
    const dir = mkdtempSync(join(tmpdir(), "sepia-meta-"));
    const path = join(dir, "meta.json");
    writeFileSync(path, JSON.stringify([{ title: "zero" }]));
    expect(createMetaStore(path).of("0")?.title).toBe("zero");
  });

  it("records run spans and round-trips them through the meta file", () => {
    const { path, store } = tempStore();
    store.addSpan("s1", { at: 1_000, agent: "devin", node: "node_a" });
    expect(store.of("s1")?.spans).toEqual([{ at: 1_000, agent: "devin", node: "node_a" }]);

    // Persisted — a fresh store over the same file sees the span.
    const reloaded = createMetaStore(path);
    expect(reloaded.of("s1")?.spans).toEqual([{ at: 1_000, agent: "devin", node: "node_a" }]);
    expect(JSON.parse(readFileSync(path, "utf8"))).toHaveProperty("sessions.s1.spans", [
      { at: 1000, agent: "devin", node: "node_a" },
    ]);
  });

  it("addSpan is idempotent on the trailing agent+node and keeps span order", () => {
    const { store } = tempStore();
    store.addSpan("s1", { at: 1, agent: "devin", node: "node_a" });
    store.addSpan("s1", { at: 2, agent: "devin", node: "node_a" }); // same run — deduped
    store.addSpan("s1", { at: 3, agent: "cline", node: "node_a" }); // new agent
    store.addSpan("s1", { at: 4, agent: "cline", node: "node_b" }); // new node
    store.addSpan("s1", { at: 5, agent: "devin", node: "node_a" }); // moved back — new span

    expect(store.of("s1")?.spans).toEqual([
      { at: 1, agent: "devin", node: "node_a" },
      { at: 3, agent: "cline", node: "node_a" },
      { at: 4, agent: "cline", node: "node_b" },
      { at: 5, agent: "devin", node: "node_a" },
    ]);
  });

  it("appendSpan returns the input unchanged on a duplicate tail", () => {
    const spans = [{ at: 1, agent: "devin", node: "n" }];
    expect(appendSpan(spans, { at: 2, agent: "devin", node: "n" })).toBe(spans);
    expect(appendSpan(undefined, { at: 1, agent: "d", node: "n" })).toEqual([
      { at: 1, agent: "d", node: "n" },
    ]);
  });

  it("normalization drops malformed span entries", () => {
    const dir = mkdtempSync(join(tmpdir(), "sepia-meta-"));
    const path = join(dir, "meta.json");
    writeFileSync(
      path,
      JSON.stringify({
        sessions: {
          s1: {
            spans: [
              { at: 1, agent: "devin", node: "node_a" },
              { at: "later", agent: "devin", node: "node_a" },
              { at: 2, agent: "", node: "node_a" },
              "nope",
              { at: 3, agent: "cline" },
            ],
          },
        },
      }),
    );
    expect(createMetaStore(path).of("s1")?.spans).toEqual([
      { at: 1, agent: "devin", node: "node_a" },
    ]);
  });

  it("creates the parent directory on first write", () => {
    const dir = mkdtempSync(join(tmpdir(), "sepia-meta-"));
    const path = join(dir, "nested", "deep", "meta.json");
    const store = createMetaStore(path);
    store.setConfig("k", 1);
    expect(readFileSync(path, "utf8")).toContain('"k":1');
  });
});

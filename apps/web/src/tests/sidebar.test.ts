import { describe, expect, it } from "vite-plus/test";
import {
  defaultSidebarSections,
  normalizeSidebarSections,
  SIDEBAR_SECTION_IDS,
  SIDEBAR_SECTION_LABELS,
  SIDEBAR_SECTION_LIMITS,
  sidebarSectionLabel,
  sidebarSectionLimit,
} from "../lib/sidebar";

describe("defaultSidebarSections", () => {
  it("matches the shipped layout: pinned → projects → sessions → folders → archived", () => {
    expect(defaultSidebarSections().map((s) => s.id)).toEqual([
      "pinned",
      "projects",
      "sessions",
      "folders",
      "archived",
    ]);
    expect(defaultSidebarSections().every((s) => s.enabled)).toBe(true);
  });
});

describe("normalizeSidebarSections", () => {
  it("returns defaults for non-array input", () => {
    expect(normalizeSidebarSections(undefined)).toEqual(defaultSidebarSections());
    expect(normalizeSidebarSections("nope")).toEqual(defaultSidebarSections());
    expect(normalizeSidebarSections({ sections: [] })).toEqual(defaultSidebarSections());
  });

  it("preserves stored order and reorders freely", () => {
    const normalized = normalizeSidebarSections([
      { id: "archived", enabled: true },
      { id: "pinned", enabled: true },
    ]);
    expect(normalized.map((s) => s.id)).toEqual([
      "archived",
      "pinned",
      "projects",
      "sessions",
      "folders",
    ]);
  });

  it("drops unknown ids, non-objects and duplicates", () => {
    const normalized = normalizeSidebarSections([
      "junk",
      { id: "bogus", enabled: true },
      { id: "pinned", enabled: false },
      { id: "pinned", enabled: true, label: "dup" },
      { id: "sessions", enabled: true },
    ]);
    expect(normalized.map((s) => s.id)).toEqual([
      "pinned",
      "sessions",
      "projects",
      "folders",
      "archived",
    ]);
    expect(normalized[0]).toMatchObject({ id: "pinned", enabled: false, label: undefined });
  });

  it("coerces fields: only explicit false disables, blank labels clear, bad limits reset", () => {
    const normalized = normalizeSidebarSections([
      { id: "pinned", enabled: 0, label: "  ", limit: "lots" },
      { id: "sessions", enabled: "yes", label: "Recent", limit: 4.7 },
      { id: "projects", enabled: true, limit: 12 },
    ]);
    expect(normalized[0]).toMatchObject({
      id: "pinned",
      enabled: true,
      label: undefined,
      limit: 5,
    });
    expect(normalized[1]).toMatchObject({
      id: "sessions",
      enabled: true,
      label: "Recent",
      limit: 4,
    });
    // limit is meaningless on non-flat sections — never carried.
    expect(normalized[2]).toMatchObject({ id: "projects", limit: undefined });
  });

  it("always covers every section id exactly once", () => {
    for (const raw of [[], [{ id: "pinned", enabled: false }], [{ id: "x" }, { id: "folders" }]]) {
      const ids = normalizeSidebarSections(raw).map((s) => s.id);
      expect([...ids].sort()).toEqual([...SIDEBAR_SECTION_IDS].sort());
      expect(new Set(ids).size).toBe(SIDEBAR_SECTION_IDS.length);
    }
  });
});

describe("label/limit helpers", () => {
  it("falls back to defaults and honors overrides", () => {
    expect(sidebarSectionLabel({ id: "folders", enabled: true })).toBe(
      SIDEBAR_SECTION_LABELS.folders,
    );
    expect(sidebarSectionLabel({ id: "folders", enabled: true, label: "Dirs" })).toBe("Dirs");
    expect(sidebarSectionLimit({ id: "archived", enabled: true })).toBe(
      SIDEBAR_SECTION_LIMITS.archived,
    );
    expect(sidebarSectionLimit({ id: "archived", enabled: true, limit: 7 })).toBe(7);
    expect(sidebarSectionLimit({ id: "folders", enabled: true })).toBeUndefined();
  });
});

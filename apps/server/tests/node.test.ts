import { mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vite-plus/test";
import { loadNodeIdentity, PROTOCOL_VERSION, SEPIA_VERSION } from "../src/node";

const tempPath = (): string => join(mkdtempSync(join(tmpdir(), "sepia-node-")), "node.json");

describe("loadNodeIdentity", () => {
  it("mints and persists an identity on first boot", () => {
    const path = tempPath();
    const identity = loadNodeIdentity(path, "my-laptop");
    expect(identity.id).toMatch(/^node_[0-9a-f]{16}$/);
    expect(identity.name).toBe("my-laptop");
    expect(identity.version).toBe(SEPIA_VERSION);

    const onDisk = JSON.parse(readFileSync(path, "utf8")) as { id: string; name: string };
    expect(onDisk).toEqual({ id: identity.id, name: "my-laptop" });
  });

  it("reads back the same id on restart — never regenerates", () => {
    const path = tempPath();
    const first = loadNodeIdentity(path, "first-name");
    const second = loadNodeIdentity(path, "second-name");
    expect(second.id).toBe(first.id);
    // The stored name wins over whatever the caller passes now.
    expect(second.name).toBe("first-name");
  });

  it("falls back to the passed name when the file lacks one", () => {
    const path = tempPath();
    writeFileSync(path, JSON.stringify({ id: "node_abc123" }));
    const identity = loadNodeIdentity(path, "fallback-name");
    expect(identity).toMatchObject({ id: "node_abc123", name: "fallback-name" });
  });

  it("a corrupt file regenerates instead of crashing", () => {
    const path = tempPath();
    writeFileSync(path, "{corrupt");
    const identity = loadNodeIdentity(path, "box");
    expect(identity.id).toMatch(/^node_[0-9a-f]{16}$/);
    expect(identity.name).toBe("box");
  });

  it("a file without a usable id regenerates", () => {
    const path = tempPath();
    writeFileSync(path, JSON.stringify({ id: 42 }));
    expect(loadNodeIdentity(path, "box").id).toMatch(/^node_[0-9a-f]{16}$/);
    writeFileSync(path, JSON.stringify("not an object"));
    expect(loadNodeIdentity(path, "box").id).toMatch(/^node_[0-9a-f]{16}$/);
  });

  it("exposes a positive protocol version", () => {
    expect(PROTOCOL_VERSION).toBeGreaterThanOrEqual(1);
  });
});

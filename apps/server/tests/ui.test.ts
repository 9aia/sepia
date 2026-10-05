/**
 * ui.ts — the embedded/directory SPA server. `Bun.file` is stubbed with a
 * Blob-backed stand-in (node has no `Bun` global); everything else is real.
 */
import { existsSync, mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vite-plus/test";
import { createUiAssets } from "../src/ui";

const realBun = (globalThis as { Bun?: unknown }).Bun;

beforeAll(() => {
  (globalThis as { Bun?: unknown }).Bun = {
    file: (path: string) => {
      const data = existsSync(path) && !statIsDir(path) ? readFileSync(path, "utf8") : "";
      const blob = new Blob([data]);
      return Object.assign(blob, {
        exists: async () => existsSync(path) && !statIsDir(path),
      });
    },
  };
});
afterAll(() => {
  (globalThis as { Bun?: unknown }).Bun = realBun;
});

import { readFileSync, statSync } from "node:fs";
const statIsDir = (path: string): boolean =>
  statSync(path, { throwIfNoEntry: false })?.isDirectory() === true;

const dir = mkdtempSync(join(tmpdir(), "sepia-ui-"));
afterAll(() => rmSync(dir, { recursive: true, force: true }));

const INDEX = "<html><body>sepia</body></html>";
const JS = "console.log(1);";

describe("createUiAssets — embedded bundle", () => {
  const indexPath = join(dir, "embedded-index.html");
  writeFileSync(indexPath, INDEX);
  const jsPath = join(dir, "embedded.js");
  writeFileSync(jsPath, JS);

  const ui = createUiAssets({
    embedded: { "/index.html": indexPath, "/assets/app.js": jsPath },
  })!;

  it("is undefined without an index.html entry", () => {
    expect(createUiAssets({})).toBeUndefined();
    expect(createUiAssets({ embedded: { "/app.js": jsPath } })).toBeUndefined();
  });

  it("serves the shell for / and extensionless SPA routes", async () => {
    const root = await ui.fetch("GET", "/");
    expect(root).not.toBeNull();
    expect(root!.headers.get("content-type")).toBe("text/html; charset=utf-8");
    expect(root!.headers.get("cache-control")).toBe("no-cache");
    expect(await root!.text()).toBe(INDEX);

    // a clean client route gets the shell — the router owns it
    const spa = await ui.fetch("GET", "/sessions/abc");
    expect(await spa!.text()).toBe(INDEX);

    const dir2 = await ui.fetch("GET", "/sessions/");
    expect(await dir2!.text()).toBe(INDEX);
  });

  it("serves hashed assets forever-cached and rejects misses/dodgy paths", async () => {
    const asset = await ui.fetch("GET", "/assets/app.js");
    expect(asset!.headers.get("content-type")).toBe("text/javascript; charset=utf-8");
    expect(asset!.headers.get("cache-control")).toContain("immutable");
    expect(await asset!.text()).toBe(JS);

    expect(await ui.fetch("GET", "/missing.js")).toBeNull();
    expect(await ui.fetch("GET", "/%E0%A4%A")).toBeNull();
    expect(await ui.fetch("GET", "/%00.json")).toBeNull();
    // `normalize` collapses the `..` before the segment check, so the path
    // resolves as extensionless "/etc/passwd" — a plain SPA fallback.
    expect(await (await ui.fetch("GET", "/../etc/passwd"))!.text()).toBe(INDEX);
  });

  it("HEAD reports the size without a body", async () => {
    const head = await ui.fetch("HEAD", "/assets/app.js");
    expect(head!.headers.get("content-length")).toBe(String(JS.length));
    expect(await head!.text()).toBe("");
  });
});

describe("createUiAssets — on-disk directory", () => {
  const site = join(dir, "site");
  mkdirSync(join(site, "assets"), { recursive: true });
  writeFileSync(join(site, "index.html"), INDEX);
  writeFileSync(join(site, "assets", "app.js"), JS);
  writeFileSync(join(site, "blob.bin"), "raw");

  const ui = createUiAssets({ dir: site })!;

  it("serves files and the SPA fallback", async () => {
    const js = await ui.fetch("GET", "/assets/app.js");
    expect(await js!.text()).toBe(JS);
    const shell = await ui.fetch("GET", "/chat/new");
    expect(await shell!.text()).toBe(INDEX);
    // unknown extension → octet-stream
    const bin = await ui.fetch("GET", "/blob.bin");
    expect(bin!.headers.get("content-type")).toBe("application/octet-stream");
    // dotfile-ish misses do not fall back
    expect(await ui.fetch("GET", "/nope.css")).toBeNull();
    // escape attempts are normalized to an in-root extensionless path —
    // contained by the router, landing on the shell
    expect(await (await ui.fetch("GET", "/%2e%2e/%2e%2e/etc/passwd"))!.text()).toBe(INDEX);
  });

  it("returns null for SPA paths when the dir has no index.html", async () => {
    const empty = join(dir, "empty-site");
    mkdirSync(empty);
    const bare = createUiAssets({ dir: empty })!;
    expect(await bare.fetch("GET", "/chat/new")).toBeNull();
  });

  it("dir: '' falls back to the embedded bundle", () => {
    expect(createUiAssets({ dir: "   ", embedded: {} })).toBeUndefined();
  });
});

describe("createUiAssets — remaining edges", () => {
  const dotless = join(dir, "LICENSE");
  writeFileSync(dotless, "license text");
  const emb = createUiAssets({
    embedded: { "/index.html": join(dir, "embedded-index.html"), "/LICENSE": dotless },
  })!;

  it("contentTypeOf falls back to octet-stream for dotless names", async () => {
    const res = await emb.fetch("GET", "/LICENSE");
    expect(res!.headers.get("content-type")).toBe("application/octet-stream");
  });

  it("directory mode rejects a malformed URI and serves / directly", async () => {
    const site = createUiAssets({ dir: join(dir, "site") })!;
    expect(await site.fetch("GET", "/%E0%A4%A")).toBeNull();
    const root = await site.fetch("GET", "/");
    expect(root!.headers.get("content-type")).toBe("text/html; charset=utf-8");
  });
});

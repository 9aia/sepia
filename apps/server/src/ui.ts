import { join, normalize, resolve, sep } from "node:path";
import { embeddedUiFiles } from "./ui.assets.gen";

/**
 * Serves the built TanStack Start SPA (apps/web/dist/client). Two sources:
 *
 * - embedded: files baked into the `bun --compile` binary via
 *   `import ... with { type: "file" }` (see tools/build-binary.ts and
 *   ui.assets.gen.ts). The record maps URL path → file path; inside a compiled
 *   binary those paths live in Bun's `/$bunfs/` virtual filesystem and plain
 *   `Bun.file()` reads them back.
 * - dir: an operator-supplied dist directory (SEPIA_UI_DIR) that overrides the
 *   embedded bundle without rebuilding the binary.
 */
export interface UiAssets {
  /** Resolves a request to a Response, or null when nothing matches. */
  readonly fetch: (method: string, pathname: string) => Promise<Response | null>;
}

const CONTENT_TYPES: Readonly<Record<string, string>> = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json",
  ".map": "application/json",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".jpeg": "image/jpeg",
  ".gif": "image/gif",
  ".webp": "image/webp",
  ".ico": "image/x-icon",
  ".woff": "font/woff",
  ".woff2": "font/woff2",
  ".ttf": "font/ttf",
  ".wasm": "application/wasm",
  ".txt": "text/plain; charset=utf-8",
  ".xml": "application/xml",
  ".webmanifest": "application/manifest+json",
};

const INDEX_PATH = "/index.html";

const contentTypeOf = (path: string): string => {
  const dot = path.lastIndexOf(".");
  if (dot === -1) return "application/octet-stream";
  return CONTENT_TYPES[path.slice(dot).toLowerCase()] ?? "application/octet-stream";
};

// Hashed bundle filenames live under /assets/ — safe to cache forever. The
// shell (index.html), manifest and service worker must revalidate.
const cacheControlOf = (path: string): string =>
  path.startsWith("/assets/") ? "public, max-age=31536000, immutable" : "no-cache";

const fileResponse = (
  file: ReturnType<typeof Bun.file>,
  path: string,
  headOnly: boolean,
): Response => {
  const headers: Record<string, string> = {
    "content-type": contentTypeOf(path),
    "cache-control": cacheControlOf(path),
  };
  if (headOnly) {
    headers["content-length"] = String(file.size);
    return new Response(null, { headers });
  }
  return new Response(file, { headers });
};

/** Rejects traversal and non-normalized paths; returns a clean "/"-rooted path. */
const sanitize = (pathname: string): string | null => {
  let decoded: string;
  try {
    decoded = decodeURIComponent(pathname);
  } catch {
    return null;
  }
  if (decoded.includes("\0")) return null;
  const normalized = normalize(decoded);
  if (!normalized.startsWith("/")) return null;
  if (normalized.split("/").includes("..")) return null;
  return normalized;
};

const hasExtension = (path: string): boolean => {
  const last = path.slice(path.lastIndexOf("/") + 1);
  return last.includes(".");
};

const embeddedAssets = (files: Readonly<Record<string, string>>): UiAssets | undefined => {
  const map = new Map(Object.entries(files));
  const index = map.get(INDEX_PATH);
  if (index === undefined) return undefined;
  return {
    fetch: async (method, pathname) => {
      const clean = sanitize(pathname);
      if (clean === null) return null;
      const head = method === "HEAD";
      const hit = map.get(clean === "/" ? INDEX_PATH : clean);
      if (hit !== undefined) {
        return fileResponse(Bun.file(hit), clean === "/" ? INDEX_PATH : clean, head);
      }
      // SPA fallback: extensionless paths (and directories) get the shell —
      // the client router owns every non-/api route.
      if (clean.endsWith("/") || !hasExtension(clean)) {
        return fileResponse(Bun.file(index), INDEX_PATH, head);
      }
      return null;
    },
  };
};

const directoryAssets = (dir: string): UiAssets => {
  const root = resolve(dir);
  const lookup = async (
    path: string,
  ): Promise<{ file: ReturnType<typeof Bun.file>; path: string } | null> => {
    const candidate = resolve(join(root, path));
    if (candidate !== root && !candidate.startsWith(root + sep)) return null;
    const file = Bun.file(candidate);
    return (await file.exists()) ? { file, path } : null;
  };
  return {
    fetch: async (method, pathname) => {
      const clean = sanitize(pathname);
      if (clean === null) return null;
      const head = method === "HEAD";
      const wanted = clean === "/" ? INDEX_PATH : clean;
      const hit = await lookup(wanted);
      if (hit !== null) return fileResponse(hit.file, wanted, head);
      if (clean.endsWith("/") || !hasExtension(clean)) {
        const index = await lookup(INDEX_PATH);
        if (index !== null) return fileResponse(index.file, INDEX_PATH, head);
      }
      return null;
    },
  };
};

export const createUiAssets = (options: {
  readonly dir?: string | undefined;
  readonly embedded?: Readonly<Record<string, string>>;
}): UiAssets | undefined => {
  if (options.dir !== undefined && options.dir.trim() !== "") {
    return directoryAssets(options.dir);
  }
  return embeddedAssets(options.embedded ?? embeddedUiFiles);
};

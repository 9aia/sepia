/**
 * Oxlint JS plugin enforcing the monorepo's module boundaries.
 *
 * Elements:
 *   - apps/<name>       — application; may use packages + own internals.
 *   - packages/<name>   — library; public surface is its index entry.
 *   - root              — anything else (configs, tests at repo root, lint/).
 *
 * Rules:
 *   sepia/no-cross-app-import   — an app may not import files inside another app.
 *   sepia/no-deep-package-import — outside a package, only its index entry is
 *                                  importable (relative or via package name).
 */
import path from "node:path";
import { existsSync } from "node:fs";

const INDEX_BASENAMES = new Set(["index.ts", "index.tsx", "index.js", "index.mjs"]);

const elementOf = (filePath, cwd) => {
  const rel = path.relative(cwd, filePath).split(path.sep).join("/");
  const parts = rel.split("/");
  if (parts[0] === "apps" && parts.length > 1) return { kind: "app", name: parts[1] };
  if (parts[0] === "packages" && parts.length > 1) return { kind: "package", name: parts[1] };
  return { kind: "root", name: null };
};

/** Resolve an import specifier to a file inside the repo, or null if external. */
const resolveImport = (spec, importerDir, cwd) => {
  if (!spec.startsWith(".") && !spec.startsWith("/")) return null;
  const base = path.resolve(importerDir, spec);
  const exts = [".ts", ".tsx", ".js", ".mjs", ".d.ts"];
  const candidates = [
    base,
    // NodeNext-style: ".js" specifiers resolve to ".ts" sources.
    ...(base.endsWith(".js") || base.endsWith(".mjs")
      ? [base.replace(/\.[jm]s$/, ".ts"), base.replace(/\.[jm]s$/, ".tsx")]
      : []),
    ...exts.map((ext) => base + ext),
    ...["index.ts", "index.tsx", "index.js"].map((f) => path.join(base, f)),
  ];
  for (const candidate of candidates) {
    if (existsSync(candidate) && candidate.startsWith(cwd)) return candidate;
  }
  return null;
};

const importVisitor = (context, check) => (node) => {
  const spec = node.source?.value;
  if (typeof spec === "string") check(node, spec);
};

const createBoundaryRule = (check) => ({
  create(context) {
    const visitor = importVisitor(context, check(context));
    return {
      ImportDeclaration: visitor,
      ExportNamedDeclaration: visitor,
      ExportAllDeclaration: visitor,
    };
  },
});

export default {
  meta: { name: "sepia" },
  rules: {
    "no-cross-app-import": createBoundaryRule((context) => {
      const cwd = context.cwd ?? process.cwd();
      const importer = elementOf(context.filename, cwd);
      if (importer.kind !== "app") return () => {};
      return (node, spec) => {
        const target = resolveImport(spec, path.dirname(context.filename), cwd);
        if (target === null) return;
        const targetElement = elementOf(target, cwd);
        if (targetElement.kind === "app" && targetElement.name !== importer.name) {
          context.report({
            node,
            message: `App '${importer.name}' must not import from app '${targetElement.name}'. Apps cannot depend on other apps — move shared code to a package.`,
          });
        }
      };
    }),

    "no-deep-package-import": createBoundaryRule((context) => {
      const cwd = context.cwd ?? process.cwd();
      const importer = elementOf(context.filename, cwd);
      return (node, spec) => {
        const target = resolveImport(spec, path.dirname(context.filename), cwd);
        if (target === null) return;
        const targetElement = elementOf(target, cwd);
        if (targetElement.kind !== "package") return;
        // Inside the same package, internal imports are fine.
        if (importer.kind === "package" && importer.name === targetElement.name) return;
        // Cross-boundary: only the package's index entry is public.
        if (!INDEX_BASENAMES.has(path.basename(target))) {
          context.report({
            node,
            message: `Deep import into package '${targetElement.name}' is not allowed. Import from the package entry point ('sepia-*' name or its index.ts).`,
          });
        }
      };
    }),

    /** apps/web is a browser bundle — the backend packages (sepia-core's
        bun:sqlite, ACP spawns) can't run there. */
    "no-backend-package-import": createBoundaryRule((context) => {
      const cwd = context.cwd ?? process.cwd();
      const importer = elementOf(context.filename, cwd);
      if (importer.kind !== "app" || importer.name !== "web") return () => {};
      return (node, spec) => {
        if (spec.startsWith("sepia-")) {
          context.report({
            node,
            message: `The web bundle must not import the backend package '${spec}'. Define the types it needs in apps/web/src/lib instead.`,
          });
        }
      };
    }),
  },
};

import { defineConfig } from "vite-plus";

export default defineConfig({
  test: {
    environment: "node",
    include: ["tests/**/*.test.ts"],
    // `*.bun.test.ts` specs import `bun:sqlite` (via src/SessionSqlite.ts and
    // src/SqliteStorage.ts) and only run under `bun test` — see `test:bun`.
    exclude: ["tests/**/*.bun.test.ts"],
    coverage: {
      provider: "v8",
      reporter: ["text", "html"],
      reportsDirectory: "coverage",
      include: ["src/**/*.ts"],
      exclude: [
        "src/index.ts",
        // bun:sqlite is imported at module load in these files, so Node can
        // never execute them; DbSchema is only consumed by SqliteStorage.
        // All three are exercised for real by tests/storage.bun.test.ts under
        // `bun test` and by the cli.integration subprocess tests.
        "src/SessionSqlite.ts",
        "src/SqliteStorage.ts",
        "src/DbSchema.ts",
      ],
      thresholds: {
        // Aggregate floor — the current total; a new uncovered file drags it
        // below the line. Per-glob entries pin each file's measured floor
        // (`vp run coverage`); raise them as coverage improves, never lower
        // them silently.
        lines: 99,
        statements: 99,
        functions: 97,
        branches: 94,
        "src/{ClineIndex,Domain,Storage}.ts": {
          lines: 100,
          statements: 100,
          functions: 100,
          branches: 100,
        },
        "src/ClaudeCode.ts": { lines: 100, statements: 100, functions: 100, branches: 99 },
        "src/ClaudeCodeRepository.ts": {
          lines: 100,
          statements: 100,
          functions: 100,
          branches: 97,
        },
        "src/Cline.ts": { lines: 100, statements: 99.5, functions: 99, branches: 94 },
        // `make`/`indexRow` run against an injected `openDb` fake under node;
        // the real `bun:sqlite` driver is covered by storage.bun.test.ts.
        "src/ClineStore.ts": { lines: 97, statements: 97, functions: 93, branches: 93 },
        "src/Conversion.ts": { lines: 97, statements: 96, functions: 93, branches: 80 },
        "src/Cursor.ts": { lines: 100, statements: 99, functions: 100, branches: 93 },
        // The uncovered lines are the default `bun:sqlite` openers' success
        // bodies — importable only under Bun (storage.bun.test.ts).
        "src/CursorRepository.ts": {
          lines: 96,
          statements: 96,
          functions: 92,
          branches: 90,
        },
        "src/ClineRepository.ts": {
          lines: 100,
          statements: 98,
          functions: 93,
          branches: 87,
        },
        "src/Devin.ts": { lines: 100, statements: 100, functions: 100, branches: 98 },
        "src/{Restore,Rewind}.ts": {
          lines: 100,
          statements: 100,
          functions: 100,
          branches: 94,
        },
      },
    },
  },
});

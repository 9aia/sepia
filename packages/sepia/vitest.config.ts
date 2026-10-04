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
        // (`bun run coverage`); raise them as coverage improves, never lower
        // them silently.
        lines: 85.9,
        statements: 84.76,
        functions: 75.55,
        branches: 81.63,
        "src/{ClineIndex,Domain,Storage}.ts": {
          lines: 100,
          statements: 100,
          functions: 100,
          branches: 100,
        },
        "src/ClaudeCode.ts": { lines: 100, statements: 100, functions: 100, branches: 97.45 },
        "src/ClaudeCodeRepository.ts": {
          lines: 100,
          statements: 98.48,
          functions: 95.45,
          branches: 90.9,
        },
        "src/Cline.ts": { lines: 91.22, statements: 91.22, functions: 88.23, branches: 81.31 },
        // Only covered by the bun-side install path (storage.bun.test.ts) and
        // the cli.integration subprocess; the injected-fake node tests never
        // call `make`.
        "src/ClineStore.ts": { lines: 10.41, statements: 10.2, functions: 0, branches: 0 },
        "src/Conversion.ts": { lines: 23.96, statements: 23.38, functions: 14.89, branches: 32.95 },
        "src/Cursor.ts": { lines: 98.41, statements: 93.2, functions: 100, branches: 80.42 },
        "src/CursorRepository.ts": {
          lines: 94.4,
          statements: 90.74,
          functions: 80.43,
          branches: 82.92,
        },
        // Node-importable but only exercised under `bun test`
        // (storage.bun.test.ts) and by apps/server's e2e suite.
        "src/ClineRepository.ts": { lines: 0, statements: 0, functions: 0, branches: 0 },
        "src/Devin.ts": { lines: 99.54, statements: 98.03, functions: 97.77, branches: 96.18 },
      },
    },
  },
});

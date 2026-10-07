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
        // Exercised by tests/storage.bun.test.ts (in sepia-convert) under bun.
        "src/SessionSqlite.ts",
        "src/SqliteStorage.ts",
        "src/DbSchema.ts",
      ],
      thresholds: {
        lines: 99,
        statements: 99,
        functions: 97,
        branches: 94,
        "src/Devin.ts": { lines: 100, statements: 100, functions: 100, branches: 98 },
        "src/DevinConfig.ts": {
          lines: 100,
          statements: 100,
          functions: 100,
          branches: 84,
        },
      },
    },
  },
});

import { defineConfig } from "vite-plus";

export default defineConfig({
  test: {
    environment: "node",
    include: ["tests/**/*.test.ts"],
    coverage: {
      provider: "v8",
      reporter: ["text", "html"],
      reportsDirectory: "coverage",
      include: ["src/**/*.ts"],
      exclude: ["src/index.ts"],
      thresholds: {
        lines: 98,
        statements: 98,
        functions: 96,
        branches: 91,
        "src/Cursor.ts": { lines: 99, statements: 99, functions: 100, branches: 93 },
        // The uncovered lines are the default `bun:sqlite` openers' success
        // bodies — importable only under Bun.
        "src/CursorRepository.ts": {
          lines: 96,
          statements: 96,
          functions: 92,
          branches: 90,
        },
      },
    },
  },
});

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
        functions: 98,
        branches: 92,
        "src/ClineIndex.ts": {
          lines: 100,
          statements: 100,
          functions: 100,
          branches: 100,
        },
        "src/Cline.ts": { lines: 100, statements: 99.5, functions: 99, branches: 94 },
        "src/ClineRepository.ts": {
          lines: 93,
          statements: 92,
          functions: 93,
          branches: 80,
        },
      },
    },
  },
});

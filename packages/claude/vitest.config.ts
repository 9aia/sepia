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
        lines: 99,
        statements: 99,
        functions: 97,
        branches: 94,
        "src/ClaudeCode.ts": { lines: 100, statements: 100, functions: 100, branches: 99 },
        "src/ClaudeCodeRepository.ts": {
          lines: 100,
          statements: 100,
          functions: 100,
          branches: 97,
        },
      },
    },
  },
});

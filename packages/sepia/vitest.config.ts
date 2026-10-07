import { defineConfig } from "vite-plus";

export default defineConfig({
  test: {
    environment: "node",
    include: ["tests/**/*.test.ts"],
    exclude: ["tests/**/*.bun.test.ts"],
    coverage: {
      provider: "v8",
      reporter: ["text", "html"],
      reportsDirectory: "coverage",
      include: ["src/**/*.ts"],
      exclude: ["src/index.ts"],
      thresholds: {
        // Aggregate floor — measured after the adapter split; raise as
        // coverage improves, never lower silently.
        lines: 97,
        statements: 97,
        functions: 97,
        branches: 90,
        "src/Domain.ts": {
          lines: 97,
          statements: 97,
          functions: 96,
          branches: 100,
        },
        "src/Storage.ts": {
          lines: 100,
          statements: 100,
          functions: 100,
          branches: 100,
        },
        "src/Shared.ts": { lines: 98, statements: 98, functions: 100, branches: 83 },
        "src/Frontmatter.ts": { lines: 95, statements: 95, functions: 100, branches: 92 },
        "src/AgentConfig.ts": { lines: 98, statements: 98, functions: 97, branches: 84 },
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

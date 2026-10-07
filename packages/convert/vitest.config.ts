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
        lines: 90,
        statements: 90,
        functions: 90,
        branches: 80,
        "src/ClineStore.ts": { lines: 97, statements: 97, functions: 93, branches: 93 },
        "src/Conversion.ts": { lines: 97, statements: 96, functions: 93, branches: 80 },
      },
    },
  },
});

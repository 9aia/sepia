import { fileURLToPath } from "node:url";
import { defineConfig } from "vite-plus";

const stub = (name: string): string =>
  fileURLToPath(new URL(`./tests/stubs/${name}`, import.meta.url));

export default defineConfig({
  resolve: {
    alias: [
      { find: /^bun:sqlite$/, replacement: stub("bun-sqlite.ts") },
      { find: /^drizzle-orm\/bun-sqlite$/, replacement: stub("drizzle-bun-sqlite.ts") },
      { find: /^drizzle-orm\/bun-sqlite\/migrator$/, replacement: stub("drizzle-bun-sqlite.ts") },
    ],
  },
  test: {
    environment: "node",
    coverage: {
      provider: "v8",
      reporter: ["text", "html"],
      reportsDirectory: "coverage",
      include: ["src/**/*.ts"],
      exclude: ["src/index.ts"],
    },
  },
});

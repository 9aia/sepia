import { fileURLToPath } from "node:url";
import { defineConfig } from "vite-plus";

const stub = (name: string): string =>
  fileURLToPath(new URL(`./tests/stubs/${name}`, import.meta.url));

export default defineConfig({
  resolve: {
    // `createApp` imports sepia-core for the convert route; its sqlite driver
    // only exists under bun, so node runs get throwing stubs. Store-backed
    // behavior stays covered by tests/e2e.ts (bun) and fake planes.
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
      exclude: ["src/main.ts"],
    },
  },
});

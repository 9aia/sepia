import { fileURLToPath } from "node:url";
import { defineConfig } from "vite-plus";

export default defineConfig({
  resolve: {
    // components import via the "@/..." alias from tsconfig paths
    alias: [{ find: /^@\//, replacement: `${fileURLToPath(new URL("./src/", import.meta.url))}` }],
  },
  test: {
    include: ["src/**/*.test.{ts,tsx}"],
    environment: "node",
    coverage: {
      provider: "v8",
      reporter: ["text", "html"],
      reportsDirectory: "coverage",
      include: ["src/**/*.ts", "src/**/*.tsx"],
      exclude: [
        "src/routeTree.gen.ts",
        "src/**/*.test.ts",
        "src/**/*.test.tsx",
        // test harness, not shipped code
        "src/test-utils/**",
        // type-only module — no runtime statements to cover
        "src/lib/types.ts",
      ],
    },
  },
});

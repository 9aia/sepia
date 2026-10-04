import { defineConfig } from "vite-plus";

/**
 * Browser e2e — real Chromium against the running stack.
 * Deliberately NOT part of `vp run -r test`: it needs `vp dev` +
 * `bun apps/server/src/main.ts` up, and it's heavy. Run manually:
 *
 *   vp run test:e2e
 */
export default defineConfig({
  test: {
    include: ["tests/e2e/**/*.spec.ts"],
    testTimeout: 60_000,
    hookTimeout: 60_000,
    // Browser tests share one page state per file — don't fork-parallel files.
    fileParallelism: false,
    reporters: ["default"],
  },
});

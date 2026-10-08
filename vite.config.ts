import { defineConfig } from "vite-plus";

export default defineConfig({
  staged: {
    "*": "vp check --fix",
  },
  fmt: {
    ignorePatterns: [
      "apps/web/src/routeTree.gen.ts",
      "apps/server/ui-dist",
      // Golden fixtures are byte-exact test data — one is intentionally
      // malformed JSON; the formatter must never touch them.
      "crates/sepia-testkit/fixtures/**",
    ],
  },
  lint: {
    // extract-golden-* are one-shot migration tooling — deep imports and
    // untyped Effect calls are inherent to generating fixtures.
    ignorePatterns: ["apps/server/ui-dist", "tools/extract-golden-*.ts"],
    jsPlugins: [
      { name: "vite-plus", specifier: "vite-plus/oxlint-plugin" },
      // eslint-plugin-boundaries can't resolve imports under oxlint (it needs
      // eslint-plugin-import's resolver); lint/boundaries.js enforces our
      // app/package boundaries textually instead.
      "./lint/boundaries.js",
    ],
    rules: {
      "vite-plus/prefer-vite-plus-imports": "error",
      "sepia/no-cross-app-import": "error",
      "sepia/no-deep-package-import": "error",
      "sepia/no-backend-package-import": "error",
    },
    options: { typeAware: true, typeCheck: true },
  },
  run: {
    cache: true,
  },
});

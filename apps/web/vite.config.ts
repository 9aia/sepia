import path from "node:path";
import { defineConfig } from "vite-plus";
import tailwindcss from "@tailwindcss/vite";
import { tanstackStart } from "@tanstack/react-start/plugin/vite";
import viteReact from "@vitejs/plugin-react";

const token = process.env.SEPIA_TOKEN;

export default defineConfig({
  resolve: {
    alias: { "@": path.resolve(__dirname, "src") },
  },
  server: {
    port: 3000,
    proxy: {
      "/api": {
        target: "http://localhost:8787",
        changeOrigin: true,
        headers:
          token === undefined || token === "" ? undefined : { authorization: `Bearer ${token}` },
      },
    },
  },
  plugins: [
    tailwindcss(),
    // SPA mode: the server build is only used at build time to prerender a
    // static shell (dist/client/index.html); every route is client-rendered,
    // so the whole UI ships as a static bundle the sepia binary embeds.
    tanstackStart({
      spa: { enabled: true, maskPath: "/", prerender: { outputPath: "index.html" } },
    }),
    viteReact(),
  ],
});

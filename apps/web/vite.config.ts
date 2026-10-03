import { defineConfig } from "vite-plus";
import tailwindcss from "@tailwindcss/vite";
import { tanstackStart } from "@tanstack/react-start/plugin/vite";
import viteReact from "@vitejs/plugin-react";

const token = process.env.SEPIA_TOKEN;

export default defineConfig({
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
  plugins: [tailwindcss(), tanstackStart(), viteReact()],
});

import { defineConfig } from "drizzle-kit";

export default defineConfig({
  dialect: "sqlite",
  schema: "./src/DbSchema.ts",
  out: "./drizzle",
  dbCredentials: {
    url: "sepia.db",
  },
});

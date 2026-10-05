#!/usr/bin/env bun
// npm entrypoint for `sepia-node`: default the UI source to the packaged web
// bundle (SEPIA_UI_DIR / SEPIA_UI=off still win), then run the bundled CLI.
import { resolve } from "node:path";

if (process.env.SEPIA_UI_DIR === undefined) {
  process.env.SEPIA_UI_DIR = resolve(import.meta.dir, "../ui");
}

await import("../dist/cli.js");

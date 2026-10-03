<!--VITE PLUS START-->

# Using Vite+, the Unified Toolchain for the Web

This project is using Vite+, a unified toolchain built on top of Vite, Rolldown, Vitest, tsdown, Oxlint, Oxfmt, and Vite Task. Vite+ wraps runtime management, package management, and frontend tooling in a single global CLI called `vp`. Vite+ is distinct from Vite, and it invokes Vite through `vp dev` and `vp build`. Run `vp help` to print a list of commands and `vp <command> --help` for information about a specific command.

Docs are local at `node_modules/vite-plus/docs` or online at https://viteplus.dev/guide/.

## Built-in Commands vs Scripts

`vp <name>` runs a built-in command. `vp run <name>` runs a `package.json` script or a `vite.config.ts` task. Scripts cannot overwrite built-ins, so `vp dev` and `vp run dev` may do different things. Check `package.json` and `vite.config.ts` first, and run `vp run <name>` when the project defines a script or task with that name.

## Tool Versions

Run `vp toolchain` to show versions and relationships in the active Vite+
release. Add a tool name to select part of the graph. For example, run
`vp toolchain vite`. Use `--global` to ignore the local `vite-plus` package. Use
`vp why <package>` to show the package-manager dependency graph.

## Review Checklist

- [ ] Run `vp install` after pulling remote changes and before getting started.
- [ ] Run `vp check` and `vp test` to format, lint, type check and test changes.
- [ ] Check if there are `vite.config.ts` tasks or `package.json` scripts necessary for validation, run via `vp run <script>`.
- [ ] If setup, runtime, or package-manager behavior looks wrong, run `vp env doctor` and include its output when asking for help.

<!--VITE PLUS END-->

# Sepia

Convert resumable coding-agent sessions through a generic session IR, and drive
them from a web UI over the Agent Client Protocol (ACP).

## Layout

| Path                       | Package                 | Role                                                                  |
| -------------------------- | ----------------------- | --------------------------------------------------------------------- |
| `packages/sepia`           | `sepia-core`            | Session IR, Devin/Cline adapters, stores, conversion                  |
| `packages/acp`             | `sepia-acp`             | Spawn an ACP agent over stdio; typed session ops + normalized updates |
| `packages/agui`            | `sepia-agui`            | Translate ACP session updates into AG-UI events; SSE encoding         |
| `packages/session-control` | `sepia-session-control` | Control plane: lists sessions, owns one live agent per session, locks |
| `apps/sepia`               | `sepia-cli`             | CLI (`list`, `import`, `export`, `install`)                           |
| `apps/server`              | `sepia-server`          | Bun API: REST + AG-UI SSE + CopilotKit runtime                        |
| `apps/web`                 | `sepia-web`             | TanStack Start UI (CopilotKit chat)                                   |
| `apps/website`             | `website`               | Unrelated Vite starter example                                        |

## Commands

```bash
vp install                # after pulling changes
vp check                  # format + lint + typecheck (root config governs all packages)
vp run -r test            # every package's tests
vp run -r build           # build the apps/packages that define a build script
vp run ready              # check + test + build

bun apps/server/src/main.ts    # API on 127.0.0.1:8787 (SEPIA_* env below)
vp run dev                     # web on :3000 (proxies /api to :8787)
bun apps/sepia/src/main.ts list --db ~/.local/share/devin/cli/sessions.db
```

## `SEPIA_*` environment

`apps/server` validates config at boot (`src/env.ts`) and fails fast on bad
values. Highlights — the full reference lives in `DEPLOY.md`:

- `SEPIA_HOST` (default `127.0.0.1`) — non-loopback binds require `SEPIA_TOKEN`.
- `SEPIA_TOKEN` — bearer auth on every `/api/*` route except `GET /api/health`.
- `SEPIA_DB` — Devin store path; opened **read-only** (`layerReadonly`).
- `SEPIA_ORIGINS`, `SEPIA_AGENT_URL`, `SEPIA_AGENT_<ID>_COMMAND`.
- `SEPIA_IDLE_TTL_MS`/`SEPIA_SWEEP_MS` — idle live-session detach.
- `SEPIA_LOCK_TTL_MS`, `SEPIA_HISTORY_LIMIT`, `SEPIA_SSE_KEEPALIVE_MS`.
- `SEPIA_INHERIT_ENV`, `SEPIA_DEBUG` — child env allowlist bypass / stderr stream.
- `COPILOTKIT_TELEMETRY_DISABLED=true` — CopilotKit telemetry opt-out.

## Dependency policy

App packages pin exact versions; `effect`, `@effect/platform`, `typescript`,
`@types/*` may use carets. Prefer versions published ≥ 7 days ago; never use
`latest`/`*`. Run `vp install` after changing deps (the lockfile is shared —
never run concurrent installs).

## Dead code candidates (not yet removed)

- `apps/website` — unrelated Vite starter.
- `packages/utils` — nothing imports it.
- `todo/repair-cline-session` — standalone legacy repair helper.

Remove these once the repo is under version control; there is no VCS today, so
deletion is irreversible.

## Constraints worth knowing

- `sepia-core` imports `bun:sqlite` at module load, so anything that imports it
  (the CLI, `sepia-server`) must run under Bun. Tests stub it via `resolve.alias`
  in the package's `vitest.config.ts`.
- `devin acp` and `cline --acp` are the agent runtimes. `devin acp` advertises
  `loadSession` plus `session/list` (with live lock metadata).
- The web UI selects a session by putting its id on the CopilotKit runtime URL:
  `/api/copilotkit?sessionId=<id>`. The runtime's agent factory turns that into
  `/api/agent?sessionId=<id>`, which is a standard AG-UI agent endpoint.
- A session locked by a live process attaches read-only; `POST .../attach` with
  `{ "takeover": true }` overrides that.
- The generated `apps/web/src/routeTree.gen.ts` is excluded from formatting via
  `fmt.ignorePatterns` in the root `vite.config.ts`.
- Only `packages/sepia` sets coverage thresholds (100%).
- The `devin acp` integration test is skipped unless `ACP_IT=1`.

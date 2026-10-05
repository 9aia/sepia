# sepia-node

The Sepia node + CLI. One `sepia serve` process per machine serves the API and
the bundled web UI on one port, lists sessions from every agent store on that
machine (Devin, Cline, Claude Code, Cursor), and drives them live over the
Agent Client Protocol (ACP).

## Install

Requires [Bun](https://bun.sh) ≥ 1.3 — the package is a bundled Bun
entrypoint, not a compiled binary:

```bash
curl -fsSL https://bun.sh/install.sh | bash   # if Bun is missing
npm i -g sepia-node                            # or: bunx sepia-node <command>
```

```bash
sepia serve          # API + web UI on 127.0.0.1:8787
sepia serve --no-ui  # API-only node
sepia version        # the release stamp (MAJOR.YYMMDD.HHMM)
sepia --help         # list / import / export / install / delete / pair
```

## Configuration

Every `SEPIA_*` variable applies (`SEPIA_HOST`, `PORT`, `SEPIA_TOKEN`,
`SEPIA_DB`, `SEPIA_CLINE_DIR`, `SEPIA_CLAUDE_DIR`, `SEPIA_CURSOR_DIR`, …).
The packaged web bundle ships at `ui/` and `bin/sepia.js` points
`SEPIA_UI_DIR` at it — override that variable to serve a different bundle, or
`SEPIA_UI=off` for an API-only node. See
[DEPLOY.md](https://github.com/9aia/sepia/blob/main/DEPLOY.md) for the full
reference.

## Multi-node

Run `sepia serve` on each machine, then pair them: `sepia pair` on a remote
node mints a one-time code; enter it plus the node URL in the UI under
Settings → Nodes → "Pair with code".

Agent CLIs are spawned by the node, not bundled: install and authenticate
`devin` (`devin acp`), `cline` (`cline --acp`), or `claude-agent-acp`
(`npm i -g @agentclientprotocol/claude-agent-acp` for Claude Code
transcripts). Override spawn argv per agent with `SEPIA_AGENT_<ID>_COMMAND`.

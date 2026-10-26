# Convert a session between agents

This guide shows how to move a session from one agent's store into
another's — a Devin session resumed by `cline --id <id>`, a Cline
session resumed by `devin -r <id>`, and so on.

Conversion is a store-to-store copy through the canonical IR: read the
session out of the source store, write it into the target agent's
layout, and hand back the id that agent resumes with. No intermediate
file, no daemon — the store verbs work straight on the stores.

## The three routes

| You want | Command |
| --- | --- |
| a session in another agent's store | `sepia install <id> --from <a> --to <b>` |
| a portable copy on disk | `sepia export <id> [out]` — IR JSON, or `--format cline` for a Cline session dir |
| a file from elsewhere in a store | `sepia import <path> --to <b>` |

`install` is the convert verb: read → IR → write, printing the id to
resume with. `export` + `import` is the same conversion split in two,
for when you want to inspect or carry the copy yourself. An
orchestrator gets the same operation as `sessions.convert` on the
[serve wire](../reference/serve-wire.md).

## Convert a Devin session

`install` defaults to `--from devin --to cline`, so the bare form
copies a Devin session into the Cline store:

```bash
sepia install roomy-relative --from devin
```

Name the target explicitly to land somewhere else:

```bash
sepia install roomy-relative --from devin --to claude
sepia install roomy-relative --from devin --to cursor
sepia install <id> --from cline --to devin
```

The Cline target says where it went and how to pick it up:

```
Installed session roomy-relative into /home/you/.cline/data
Resume it with: cline --id roomy-relative -m <model>
```

What each target writes, and how the agent resumes it:

| Target | Written | Resume with |
| --- | --- | --- |
| `cline` | `<data-dir>/sessions/<id>/` (manifest + transcript) plus the index row | `cline --id <id>` |
| `claude` | `<claude-dir>/projects/<cwd-slug>/<id>.jsonl` | Claude Code's own resume over that project |
| `cursor` | `<cursor-dir>/projects/<cwd-slug>/agent-transcripts/<id>/` plus `chats/<hash>/<id>/` | `cursor-agent --resume` from that project |
| `devin` | `sessions`, `message_nodes`, `prompt_history` rows | `devin -r <id>` |

The Cline target also drops `<id>.sepia.json` next to the manifest: the
IR sidecar sepia reads back from. Cline itself only loads the manifest
and transcript, so the sidecar is invisible to it.

The source is only read: converting never edits the session you
converted from (Devin's database is opened read-only).

## Store selection and the default dirs

`--from`/`--to` name a store. `--db` sets where Devin's SQLite lives;
the three `--*-dir` flags both point at a store **and** select it when
the corresponding role is silent — which is why a single `--data-dir`
is enough to write Cline layout into a scratch directory.

| Store | Path flag | Default |
| --- | --- | --- |
| `devin` | `--db` | `~/.local/share/devin/cli/sessions.db` |
| `cline` | `--data-dir` | `~/.cline/data` |
| `claude` | `--claude-dir` | `~/.claude` |
| `cursor` | `--cursor-dir` | `~/.cursor` |

When both forms appear, the explicit `--from`/`--to` wins; the path
flag only decides the role that flag left unnamed.

Convert into a scratch directory first — it costs nothing and shows
exactly what would land in a real agent store:

```bash
mkdir -p /tmp/probe
sepia install roomy-relative --from devin --data-dir /tmp/probe
sepia list --data-dir /tmp/probe
```

## Ids, collisions, `--force`

- `--id <new-id>` installs under a different id. Without it `claude`,
  `cursor` and `devin` keep the session's own id — which is what makes
  a converted copy answer to the same `devin -r <id>` — while a Cline
  target derives a fresh id from the session's creation time and prints
  the one it chose.
- `claude` and `cursor` refuse an id that already exists in the target
  store — `pass --force to overwrite`.
- A `devin` target leaves an existing session alone and says so
  (`already imported; leaving it untouched`); `--force` does not
  overwrite there.
- `--force` also lets a Cline install replace a session row a live
  session still owns.

## What survives the trip

Everything the IR models: the node forest (branches, not a flat list),
roles, text and non-text blocks, thinking **with its provider
signature**, tool calls with arguments, status, locations and diffs,
tool results with exit codes and timings, prompt history, checkpoints,
and the meta overlay (title, working directory, model). Signatures are
replayed verbatim so the target's model can keep using the transcript;
fields a target store cannot express are marked `lossy` in metadata
instead of dropped — see [session formats](../reference/session-formats.md).

Transcripts are copied whole, so the cost scales with the source: a
40k-node Devin session lands as roughly 540 MB of Claude JSONL and
converts in seconds. Budget disk, not patience.

## Agent configs are a separate install

`sepia install` moves *sessions*. Rules, skills, commands, hooks,
subagents and MCP servers convert with
`sepia config install --from <a> --to <b>` — see
[agent configs](../reference/agent-configs.md).

## Check the result

```bash
sepia list --from claude                     # or --data-dir / --cursor-dir
sepia export roomy-relative --from claude | head -c 400
```

Listing it back with the right title and working directory means the
target store's layout is right; `export` shows the IR that agent will
hand back on the next read.

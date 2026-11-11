# Distribute agent configs with tamias

`type: sepia-config` packs carry [`AgentConfig`](../reference/) IR
documents — skills, rules, commands, hooks, subagents — and deploy
them into each agent's config dir through the same write path as
`sepia config write`. One `tamias install` provisions the same
profile on every agent on the machine.

## Pack layout

```text
my-profile/
  tamias.yaml            # manifest + sepia: section
  configs/
    profile.json         # AgentConfig doc (.json or .toml)
```

```yaml
name: my-profile
type: sepia-config
version: "0.1.0"

sepia:
  agents:
    claude: configs/profile.json
    cursor: [configs/base.json, configs/cursor.json]  # sequential merge
    all: configs/base.json                            # fallback key
```

Agent keys: `claude`, `cursor`, `cline`, `devin`, `all` (`all`
fills agents without an explicit entry). Target dirs default like
the CLI — `~/.claude`, `~/.cursor`, cwd, `~/.config/devin` — and are
overridable per agent with `--<agent>-dir`.

## Install / remove

```bash
tamias install <ref>           # resolves tamias-driver-sepia-config
# or directly:
tamias-driver-sepia-config deploy --dir my-profile --name my-profile

tamias-driver-sepia-config uninstall --name my-profile
```

`deploy` records every written file (action + sha256) under
`$SEPIA_HOME/tamias/sepia-config/<pack>.json`. `uninstall` removes
only files the pack *created* whose digest still matches — merged
files (e.g. the managed `CLAUDE.md` block) are reported and kept.

An example pack lives at `examples/sepia-config-profile/`.

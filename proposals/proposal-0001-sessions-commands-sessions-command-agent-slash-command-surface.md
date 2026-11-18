+++
id = "P-001"
title = "sessions.commands + sessions.command — agent slash-command surface"
status = "in-progress"
created = "2026-11-18"
owner = "devin"
priority = "med"
labels = ["acp", "sepia"]
+++

## Context

Orchestrators (peregrin campaigns, ops drivers) need to trigger agent slash commands — `/compact` being the immediate case (long-running sessions hitting context limits). Today the only path is hardcoding `sessions.prompt` text, which is agent-specific and conflates a control operation with a content turn.

## Proposal

Expose the ACP-advertised command surface generically — NOT a per-command op:

- `sessions.commands` → the agent's advertised slash commands (from `available_commands_update`, already parsed in sepia-acp types as `AvailableCommand{name,description}`).
- `sessions.command {session_id, name, args?}` → invoke one advertised command; resolves the wire mechanism (prompt-text vs dedicated ACP method — VERIFY against `devin acp` first), awaits terminal state like `prompt_wait`, returns a typed result.

`compact` becomes `command("compact")` — no special-casing. Unadvertised commands return a clean `unsupported`. Compaction boundary events remain observable via session updates/history — no fake "did it compact" semantics needed.

## Boundary

sepia owns mechanism: what commands exist + how to send them. Policy (compact when context > threshold, which agent, retry) stays in peregrin/campaigns.

## Open questions

- Invocation mechanism per agent: prompt-text vs dedicated ACP method — probe `devin acp` `availableCommands` + a live `/compact` before implementing.
- Result shape: reuse `PromptWaitResult` or a slimmer `CommandResult`.
- Entity-plane mirror: `sepia.command`/`sepia.commands` ops in sepia-peregrin (same pattern as `sepia.prompt_wait`).
- Whether `sessions.commands` should be cached per attach or fetched live per call.

## Non-goals

No `sessions.compact` verb — command surface covers it and every future agent command for free.

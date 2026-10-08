//! Agent registry — the built-in agent specs and per-agent spawn flags.

use crate::types::AgentSpec;

pub fn builtin_agents() -> Vec<AgentSpec> {
    vec![
        AgentSpec {
            id: "devin".into(),
            label: "Devin".into(),
            command: vec!["devin".into(), "acp".into()],
            env: None,
        },
        AgentSpec {
            id: "cline".into(),
            label: "Cline".into(),
            command: vec!["cline".into(), "--acp".into()],
            env: None,
        },
        // Claude Code has no native ACP mode; the agentclientprotocol
        // adapter bridges it and supports session/load + session/list over
        // the JSONL transcripts.
        AgentSpec {
            id: "claude".into(),
            label: "Claude Code".into(),
            command: vec!["claude-agent-acp".into()],
            env: None,
        },
    ]
}

pub fn resolve_agent(id: &str, overrides: &[AgentSpec]) -> Result<AgentSpec, String> {
    if let Some(spec) = overrides.iter().find(|a| a.id == id) {
        return Ok(spec.clone());
    }
    if let Some(spec) = builtin_agents().into_iter().find(|a| a.id == id) {
        return Ok(spec);
    }
    let known = overrides
        .iter()
        .chain(builtin_agents().iter())
        .map(|a| a.id.clone())
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "Unknown agent \"{id}\"{}",
        if known.is_empty() {
            String::new()
        } else {
            format!("; known agents: {known}")
        }
    ))
}

/// Per-agent spawn flags for model selection — appended to the agent's
/// command at spawn time. devin takes fuzzy names + an ordered refusal
/// fallback list; cline takes a single `-m` model id; claude's ACP
/// adapter picks the model inside the session, not on argv.
pub fn model_args(
    agent_id: &str,
    model: Option<&str>,
    fallbacks: Option<&[String]>,
) -> Vec<String> {
    let mut args = Vec::new();
    if agent_id == "cline" {
        if let Some(m) = model {
            args.extend(["-m".into(), m.to_string()]);
        }
        return args;
    }
    if agent_id == "claude" {
        return args;
    }
    if let Some(m) = model {
        args.extend(["--model".into(), m.to_string()]);
    }
    if agent_id == "devin" {
        if let Some(fb) = fallbacks {
            if !fb.is_empty() {
                args.extend(["--refusal-fallback".into(), fb.join(",")]);
            }
        }
    }
    args
}

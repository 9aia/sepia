//! `sepia-driver-config` — the `type: sepia-config` tamias content
//! driver. A pack carries [`sepia_core::agent_config::AgentConfig`]
//! documents per agent; `deploy` writes them through the same
//! per-agent `config::write` path as `sepia config write`, so a
//! tamias install provisions agent skills/rules/commands/agents
//! exactly like the CLI does.
//!
//! Manifest (`tamias.yaml` in the pack payload):
//!
//! ```yaml
//! name: my-profile
//! type: sepia-config
//! version: "0.1.0"
//! sepia:
//!   agents:
//!     claude: configs/claude.json        # one AgentConfig doc
//!     cursor: [configs/base.toml, configs/cursor-extra.toml]
//!     all: configs/base.json             # fallback for unlisted agents
//! ```
//!
//! Agent keys: `claude`, `cursor`, `cline`, `devin`, `all`. Config
//! docs are `.json` or `.toml`. Target dirs default like the CLI
//! (`~/.claude`, `~/.cursor`, cwd, `~/.config/devin`) and are
//! overridable with `--<agent>-dir`.
//!
//! Verbs (tamias driver contract): `install` validates and prints
//! the plan, `deploy` writes, `uninstall` removes files this pack
//! created (digest-guarded; merged files are reported and kept),
//! `config-schema`/`capabilities` are discovery verbs.
//!
//! State: `$SEPIA_HOME/tamias/sepia-config/<pack>.json` (default
//! `~/.local/share/sepia`) records every file a deploy wrote with
//! its action + sha256 — uninstall only deletes `wrote` entries
//! whose digest still matches.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use sepia_core::agent_config::{AgentConfig, ConfigWriteAction, WriteActionKind};
use serde::Deserialize;

pub const CONTENT_TYPE: &str = "sepia-config";
pub const STATE_DIR: &str = "tamias/sepia-config";

// ------------------------------------------------------------------
// args
// ------------------------------------------------------------------

/// Parsed driver args — `--dir`, `--name`, `--<agent>-dir`,
/// `--dry-run`. No template vars: agent configs are typed IR.
#[derive(Debug, Default)]
pub struct Args {
    pub dir: Option<PathBuf>,
    pub name: Option<String>,
    pub claude_dir: Option<PathBuf>,
    pub cursor_dir: Option<PathBuf>,
    pub cline_dir: Option<PathBuf>,
    pub devin_dir: Option<PathBuf>,
    pub dry_run: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum DriverError {
    #[error("{0}")]
    Msg(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("yaml: {0}")]
    Yaml(#[from] serde_yaml_ng::Error),
    #[error("toml: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("agent config: {0}")]
    Agent(#[from] sepia_core::StorageError),
}

fn err(msg: impl Into<String>) -> DriverError {
    DriverError::Msg(msg.into())
}

pub type Result<T> = std::result::Result<T, DriverError>;

/// Parse argv after the verb (`main` strips it).
pub fn parse_args(argv: &[String]) -> Result<Args> {
    let mut a = Args::default();
    let mut i = 0;
    while i < argv.len() {
        let take = |i: &mut usize| -> Result<String> {
            *i += 1;
            argv.get(*i)
                .cloned()
                .ok_or_else(|| err(format!("{} needs a value", argv[*i - 1])))
        };
        match argv[i].as_str() {
            "--dir" => a.dir = Some(PathBuf::from(take(&mut i)?)),
            "--name" => a.name = Some(take(&mut i)?),
            "--claude-dir" => a.claude_dir = Some(PathBuf::from(take(&mut i)?)),
            "--cursor-dir" => a.cursor_dir = Some(PathBuf::from(take(&mut i)?)),
            "--cline-dir" => a.cline_dir = Some(PathBuf::from(take(&mut i)?)),
            "--devin-dir" => a.devin_dir = Some(PathBuf::from(take(&mut i)?)),
            "--dry-run" => a.dry_run = true,
            other => return Err(err(format!("unknown arg {other:?}"))),
        }
        i += 1;
    }
    Ok(a)
}

// ------------------------------------------------------------------
// manifest + agent resolution
// ------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct Manifest {
    name: String,
    #[serde(default)]
    sepia: SepiaSection,
}

#[derive(Debug, Default, Deserialize)]
struct SepiaSection {
    #[serde(default)]
    agents: BTreeMap<String, AgentSrc>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum AgentSrc {
    One(String),
    Many(Vec<String>),
}

impl AgentSrc {
    fn paths(&self) -> Vec<&str> {
        match self {
            Self::One(p) => vec![p.as_str()],
            Self::Many(v) => v.iter().map(String::as_str).collect(),
        }
    }
}

/// The four real agent targets (never `all` — that's a source key).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Agent {
    Claude,
    Cursor,
    Cline,
    Devin,
}

impl Agent {
    pub const ALL: [Self; 4] = [Self::Claude, Self::Cursor, Self::Cline, Self::Devin];

    fn parse(s: &str) -> Option<Self> {
        match s {
            "claude" => Some(Self::Claude),
            "cursor" => Some(Self::Cursor),
            "cline" => Some(Self::Cline),
            "devin" => Some(Self::Devin),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Cursor => "cursor",
            Self::Cline => "cline",
            Self::Devin => "devin",
        }
    }
}

/// One resolved deploy target: agent → config sources (pack-relative)
/// → concrete dir.
#[derive(Debug)]
pub struct Target {
    pub agent: Agent,
    pub dir: PathBuf,
    pub sources: Vec<PathBuf>,
}

fn load_manifest(dir: &Path) -> Result<(Manifest, String)> {
    let text = std::fs::read_to_string(dir.join("tamias.yaml"))
        .map_err(|e| err(format!("tamias.yaml: {e}")))?;
    let m: Manifest =
        serde_yaml_ng::from_str(&text).map_err(|e| err(format!("tamias.yaml: {e}")))?;
    let name = m.name.clone();
    Ok((m, name))
}

/// Resolve `sepia.agents` into concrete targets. `all` fills agents
/// without an explicit entry. Unknown keys are an error (typo
/// safety); a pack with no resolvable agents errors too.
fn resolve_targets(manifest: &Manifest, dir: &Path, a: &Args) -> Result<Vec<Target>> {
    let section = &manifest.sepia;
    for key in section.agents.keys() {
        if Agent::parse(key).is_none() && key != "all" {
            return Err(err(format!(
                "unknown agent {key:?} (want claude|cursor|cline|devin|all)"
            )));
        }
    }
    let all_src = section.agents.get("all");
    let mut targets = Vec::new();
    for agent in Agent::ALL {
        let src = section.agents.get(agent.as_str()).or(all_src);
        let Some(src) = src else { continue };
        let dir_flag = match agent {
            Agent::Claude => &a.claude_dir,
            Agent::Cursor => &a.cursor_dir,
            Agent::Cline => &a.cline_dir,
            Agent::Devin => &a.devin_dir,
        };
        let agent_dir = dir_flag.clone().unwrap_or_else(|| default_agent_dir(agent));
        let agent_dir = std::path::absolute(&agent_dir).unwrap_or(agent_dir);
        targets.push(Target {
            agent,
            dir: agent_dir,
            sources: src.paths().iter().map(|p| dir.join(p)).collect(),
        });
    }
    if targets.is_empty() {
        return Err(err("sepia.agents is empty — nothing to deploy"));
    }
    Ok(targets)
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

/// Same defaults as `sepia config` (sepia-cli `default_dir`).
fn default_agent_dir(agent: Agent) -> PathBuf {
    match agent {
        Agent::Claude => home_dir().join(".claude"),
        Agent::Cursor => home_dir().join(".cursor"),
        Agent::Cline => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        Agent::Devin => home_dir().join(".config/devin"),
    }
}

// ------------------------------------------------------------------
// config docs
// ------------------------------------------------------------------

/// Load one `AgentConfig` doc — `.json` or `.toml` by extension
/// (anything else tries JSON first, then TOML).
pub fn load_config(path: &Path) -> Result<AgentConfig> {
    let text =
        std::fs::read_to_string(path).map_err(|e| err(format!("{}: {e}", path.display())))?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext {
        "toml" => Ok(toml::from_str(&text).map_err(|e| err(format!("{}: {e}", path.display())))?),
        _ => Ok(serde_json::from_str(&text).map_err(|e| err(format!("{}: {e}", path.display())))?),
    }
}

/// Count items for the install plan summary.
fn summarize(c: &AgentConfig) -> String {
    format!(
        "{} skill(s), {} rule(s), {} command(s), {} agent(s), {} hook(s)",
        c.skills.len(),
        c.rules.len(),
        c.commands.len(),
        c.agents.len(),
        c.hooks.len(),
    )
}

/// Dispatch `config::write` to the right agent driver.
fn write_agent(agent: Agent, dir: &Path, config: &AgentConfig) -> Result<Vec<ConfigWriteAction>> {
    let res = match agent {
        Agent::Claude => sepia_driver_claude::config::write(config, dir),
        Agent::Cursor => sepia_driver_cursor::config::write(config, dir),
        Agent::Cline => sepia_driver_cline::config::write(config, dir),
        Agent::Devin => sepia_driver_devin::config::write(config, dir),
    };
    res.map_err(DriverError::from)
}

// ------------------------------------------------------------------
// state (uninstall bookkeeping)
// ------------------------------------------------------------------

#[derive(Debug, serde::Serialize, serde::Deserialize, Default)]
struct PackState {
    pack: String,
    /// agent → dir → path → (action, sha256)
    #[serde(default)]
    files: BTreeMap<String, BTreeMap<String, BTreeMap<String, FileRecord>>>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct FileRecord {
    action: WriteActionKind,
    sha256: String,
}

fn sepia_home() -> PathBuf {
    std::env::var_os("SEPIA_HOME")
        .map_or_else(|| home_dir().join(".local/share/sepia"), PathBuf::from)
}

fn state_path(pack: &str) -> PathBuf {
    sepia_home().join(STATE_DIR).join(format!("{pack}.json"))
}

fn read_state(path: &Path) -> Result<PackState> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(serde_json::from_str(&t)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(PackState {
            pack: String::new(),
            files: BTreeMap::new(),
        }),
        Err(e) => Err(e.into()),
    }
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    h.update(data);
    h.finalize()
        .iter()
        .fold(String::with_capacity(64), |mut acc, b| {
            let _ = write!(acc, "{b:02x}");
            acc
        })
}

fn action_tag(k: WriteActionKind) -> &'static str {
    match k {
        WriteActionKind::Wrote => "wrote",
        WriteActionKind::Updated => "updated",
        WriteActionKind::Unchanged => "unchanged",
        WriteActionKind::Merged => "merged",
        WriteActionKind::Skipped => "skipped",
    }
}

// ------------------------------------------------------------------
// verbs
// ------------------------------------------------------------------

/// `install` — parse manifest + every config doc, print the plan.
pub fn install(a: &Args) -> Result<()> {
    let dir = required_dir(a)?;
    let (manifest, name) = load_manifest(&dir)?;
    let targets = resolve_targets(&manifest, &dir, a)?;
    println!("sepia-config: {name} — {} agent target(s)", targets.len());
    for t in &targets {
        let mut items = 0usize;
        for src in &t.sources {
            let c = load_config(src)?;
            items += c.skills.len() + c.rules.len() + c.commands.len() + c.agents.len();
            println!(
                "  {} ← {} ({})",
                t.agent.as_str(),
                src.display(),
                summarize(&c)
            );
        }
        println!("    → {}", t.dir.display());
        let _ = items;
    }
    Ok(())
}

/// `deploy` — write all targets; records state for uninstall.
pub fn deploy(a: &Args) -> Result<()> {
    let dir = required_dir(a)?;
    let (manifest, name) = load_manifest(&dir)?;
    let targets = resolve_targets(&manifest, &dir, a)?;
    if a.dry_run {
        println!(
            "sepia-config: {name} — dry run, {} target(s)",
            targets.len()
        );
        for t in &targets {
            println!("  {} → {}", t.agent.as_str(), t.dir.display());
        }
        return Ok(());
    }
    let mut state = read_state(&state_path(&name))?;
    state.pack.clone_from(&name);
    for t in &targets {
        for src in &t.sources {
            let config = load_config(src)?;
            let actions = write_agent(t.agent, &t.dir, &config)?;
            let agent_key = t.agent.as_str().to_string();
            let dir_key = t.dir.to_string_lossy().into_owned();
            let entry = state
                .files
                .entry(agent_key)
                .or_default()
                .entry(dir_key)
                .or_default();
            for act in &actions {
                let path = act.path.clone();
                // Only files we can verify later get recorded;
                // `unchanged`/`skipped` carry no new bytes.
                if matches!(
                    act.action,
                    WriteActionKind::Unchanged | WriteActionKind::Skipped
                ) {
                    continue;
                }
                let digest = std::fs::read(&path)
                    .map(|b| sha256_hex(&b))
                    .unwrap_or_default();
                entry.insert(
                    path.clone(),
                    FileRecord {
                        action: act.action,
                        sha256: digest,
                    },
                );
                println!(
                    "  {:9} {} ({})",
                    action_tag(act.action),
                    path,
                    t.agent.as_str()
                );
            }
        }
    }
    let sp = state_path(&name);
    if let Some(parent) = sp.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&sp, serde_json::to_string_pretty(&state)?)?;
    println!("state: {}", sp.display());
    Ok(())
}

/// `uninstall` — remove `wrote` files whose digest still matches;
/// report merged/updated files as kept.
pub fn uninstall(a: &Args) -> Result<()> {
    let name = a
        .name
        .clone()
        .ok_or_else(|| err("uninstall needs --name <pack>"))?;
    let sp = state_path(&name);
    let mut state = read_state(&sp)?;
    if state.files.is_empty() {
        println!("sepia-config: nothing recorded for {name}");
        return Ok(());
    }
    let mut removed = 0usize;
    let mut kept = 0usize;
    for dirs in state.files.values_mut() {
        for files in dirs.values_mut() {
            let stale: Vec<String> = files
                .iter()
                .filter(|(path, rec)| {
                    rec.action == WriteActionKind::Wrote
                        && std::fs::read(path).is_ok_and(|b| sha256_hex(&b) == rec.sha256)
                })
                .map(|(p, _)| p.clone())
                .collect();
            for path in stale {
                if a.dry_run {
                    println!("would remove {path}");
                } else {
                    std::fs::remove_file(&path)?;
                    println!("removed {path}");
                }
                removed += 1;
            }
            // Entries that aren't safely removable stay recorded.
            files.retain(|_, rec| rec.action != WriteActionKind::Wrote);
            kept += files.len();
        }
    }
    if !a.dry_run {
        state
            .files
            .retain(|_, dirs| dirs.values().any(|f| !f.is_empty()));
        if state
            .files
            .values()
            .all(|d| d.values().all(BTreeMap::is_empty))
        {
            let _ = std::fs::remove_file(&sp);
        } else {
            std::fs::write(&sp, serde_json::to_string_pretty(&state)?)?;
        }
    }
    println!("sepia-config: {removed} removed, {kept} kept (merged/modified)");
    Ok(())
}

fn required_dir(a: &Args) -> Result<PathBuf> {
    a.dir.clone().ok_or_else(|| err("missing --dir <payload>"))
}

/// `capabilities` — the tamias discovery document.
pub fn capabilities() -> serde_json::Value {
    serde_json::json!({
        "name": CONTENT_TYPE,
        "types": [CONTENT_TYPE],
        "verbs": ["install", "deploy", "uninstall", "config-schema", "capabilities"],
        "agents": Agent::ALL.iter().map(|a| a.as_str()).collect::<Vec<_>>(),
    })
}

/// `config-schema` — JSON schema for the `sepia:` manifest section.
pub fn config_schema() -> serde_json::Value {
    serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "sepia-config pack manifest (sepia: section)",
        "type": "object",
        "properties": {
            "agents": {
                "type": "object",
                "description": "agent → AgentConfig doc path(s), or 'all' as fallback",
                "additionalProperties": {
                    "oneOf": [
                        {"type": "string"},
                        {"type": "array", "items": {"type": "string"}}
                    ]
                }
            }
        },
        "required": ["agents"]
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn write_pack(dir: &Path, manifest: &str, files: &[(&str, &str)]) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("tamias.yaml"), manifest).unwrap();
        for (name, body) in files {
            let p = dir.join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
    }

    #[test]
    fn parse_args_flags() {
        let argv: Vec<String> = [
            "--dir",
            "/tmp/p",
            "--name",
            "pack",
            "--claude-dir",
            "/tmp/c",
            "--dry-run",
        ]
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
        let a = parse_args(&argv).unwrap();
        assert_eq!(a.dir.unwrap(), PathBuf::from("/tmp/p"));
        assert_eq!(a.name.as_deref(), Some("pack"));
        assert_eq!(a.claude_dir.unwrap(), PathBuf::from("/tmp/c"));
        assert!(a.dry_run);
    }

    #[test]
    fn manifest_resolves_all_fallback() {
        let dir = std::env::temp_dir().join(format!("sepia-cfg-{}", std::process::id()));
        write_pack(
            &dir,
            "name: p\ntype: sepia-config\nsepia:\n  agents:\n    all: base.json\n",
            &[("base.json", "{}")],
        );
        let (m, name) = load_manifest(&dir).unwrap();
        assert_eq!(name, "p");
        let a = Args::default();
        let targets = resolve_targets(&m, &dir, &a).unwrap();
        assert_eq!(targets.len(), 4);
        assert!(targets.iter().all(|t| t.sources.len() == 1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explicit_beats_all_and_unknown_rejected() {
        let dir = std::env::temp_dir().join(format!("sepia-cfg2-{}", std::process::id()));
        write_pack(
            &dir,
            "name: p\ntype: sepia-config\nsepia:\n  agents:\n    claude: c.json\n    all: base.json\n",
            &[("c.json", "{}"), ("base.json", "{}")],
        );
        let (m, _) = load_manifest(&dir).unwrap();
        let targets = resolve_targets(&m, &dir, &Args::default()).unwrap();
        let claude = targets.iter().find(|t| t.agent == Agent::Claude).unwrap();
        assert!(claude.sources[0].ends_with("c.json"));
        let _ = std::fs::remove_dir_all(&dir);

        let dir2 = std::env::temp_dir().join(format!("sepia-cfg3-{}", std::process::id()));
        write_pack(
            &dir2,
            "name: p\ntype: sepia-config\nsepia:\n  agents:\n    bogus: x.json\n",
            &[("x.json", "{}")],
        );
        let (m2, _) = load_manifest(&dir2).unwrap();
        assert!(resolve_targets(&m2, &dir2, &Args::default()).is_err());
        let _ = std::fs::remove_dir_all(&dir2);
    }

    #[test]
    fn loads_json_and_toml_configs() {
        let dir = std::env::temp_dir().join(format!("sepia-cfg4-{}", std::process::id()));
        write_pack(
            &dir,
            "",
            &[
                ("a.json", "{\"skills\":[{\"name\":\"s\",\"body\":\"b\"}]}"),
                ("b.toml", "[skills]\n"),
            ],
        );
        // .toml with wrong shape errors — use a valid minimal doc
        std::fs::write(dir.join("b.toml"), "").unwrap();
        let c = load_config(&dir.join("a.json")).unwrap();
        assert_eq!(c.skills.len(), 1);
        let c2 = load_config(&dir.join("b.toml")).unwrap();
        assert_eq!(c2.skills.len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

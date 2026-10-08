//! `sepia driver list` — the local discovery view: every
//! `sepia-driver-*` binary [`sepia_driver_host::DriverRegistry`] finds
//! (`$SEPIA_DRIVER_DIR` → `~/.local/share/sepia/drivers` →
//! `/usr/local/lib/sepia/drivers` → PATH), probed via `--manifest`.
//! No running node involved — this is what the node would load at boot.

use sepia_driver_host::{DriverRegistry, command_resolves};
use sepia_driver_sdk::manifest::Capability;
use serde_json::{Value, json};

use crate::CliError;

/// One capability as a compact label — the camelCase wire name for the
/// unit variants, `convert:{from}->{to}` for a conversion pair.
fn capability_label(capability: &Capability) -> String {
    match capability {
        Capability::Convert { from, to } => format!("convert:{from}->{to}"),
        other => serde_json::to_value(other)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default(),
    }
}

/// The agent-command column: `-` when the driver advertises none, the
/// command itself when it resolves, `{cmd} (missing)` when it doesn't.
fn agent_label(manifest: &sepia_driver_sdk::manifest::DriverManifest) -> String {
    match &manifest.agent_command {
        None => "-".to_string(),
        Some(cmd) if command_resolves(cmd) => cmd.clone(),
        Some(cmd) => format!("{cmd} (missing)"),
    }
}

/// `sepia driver list` — `id<TAB>path<TAB>version<TAB>capabilities<TAB>agent`
/// rows like the other list verbs (one row per line, tab-separated so
/// non-tty output stays parseable). Probe problems warn on stderr and
/// never fail the listing — same policy as the node's boot discovery.
pub fn list(rt: &tokio::runtime::Runtime, json_out: bool) -> Result<(), CliError> {
    let (registry, problems) = rt.block_on(DriverRegistry::discover());
    for problem in &problems {
        eprintln!("warning: {problem}");
    }
    let drivers: Vec<(String, &sepia_driver_sdk::manifest::DriverManifest)> = registry
        .manifests()
        .map(|m| {
            (
                registry
                    .entry(&m.id)
                    .map_or_else(String::new, |e| e.binary.display().to_string()),
                m,
            )
        })
        .collect();
    if json_out {
        let value = json!({
            "drivers": drivers.iter().map(|(binary, m)| {
                let mut v = serde_json::to_value(m).unwrap_or_else(|_| Value::Null);
                if let Value::Object(ref mut o) = v {
                    o.insert("binary".into(), json!(binary));
                    o.insert(
                        "agentCommandResolves".into(),
                        json!(m.agent_command.as_ref().is_none_or(|c| command_resolves(c))),
                    );
                }
                v
            }).collect::<Vec<_>>(),
            "problems": problems,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&value).unwrap_or_default()
        );
        return Ok(());
    }
    if drivers.is_empty() {
        println!("No drivers found");
        return Ok(());
    }
    for (binary, manifest) in &drivers {
        let capabilities = manifest
            .capabilities
            .iter()
            .map(capability_label)
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "{}\t{}\t{}\t{}\t{}",
            manifest.id,
            binary,
            manifest.version,
            capabilities,
            agent_label(manifest)
        );
    }
    Ok(())
}

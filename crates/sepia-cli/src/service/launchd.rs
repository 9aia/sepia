//! Port of `apps/sepia/src/service/launchd.ts` — one `ai.sepia` job: a
//! LaunchAgent under `~/Library/LaunchAgents` for `--user` (the only
//! sensible default for a dev tool) or a LaunchDaemon under
//! `/Library/LaunchDaemons` for `--system`. Unlike systemd, launchd has
//! no EnvironmentFile directive, so the env file is rendered into the
//! plist's `EnvironmentVariables` dict at install time — edit the file,
//! re-run `sepia service install` to apply.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::{CliError, ServiceSpec, ServiceStatus, env_template};

/// `ai.sepia` for the node, `ai.sepia-hub` for the hub.
fn label(spec: &ServiceSpec) -> String {
    format!("ai.{}", spec.name.replace('-', "."))
}
/// `status` shows the last N log lines; `logs` prints the last N.
const STATUS_LOG_LINES: usize = 10;
const LOG_LINES: usize = 50;

fn uid() -> u32 {
    nix::unistd::getuid().as_raw()
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

/// The launchd domain a spec loads into: `gui/<uid>` or `system`.
fn domain(spec: &ServiceSpec) -> String {
    if spec.system {
        "system".into()
    } else {
        format!("gui/{}", uid())
    }
}

/// The service endpoint `print`/`kickstart` take: `gui/<uid>/ai.sepia`.
fn service_target(spec: &ServiceSpec) -> String {
    format!("{}/{}", domain(spec), label(spec))
}

fn log_path(spec: &ServiceSpec) -> PathBuf {
    if spec.system {
        PathBuf::from(format!("/var/log/{}.log", spec.name))
    } else {
        home_dir().join(format!("Library/Logs/{}.log", spec.name))
    }
}

/// Where the plist file lives for this spec.
pub fn unit_path(spec: &ServiceSpec) -> PathBuf {
    if spec.system {
        PathBuf::from(format!("/Library/LaunchDaemons/{}.plist", label(spec)))
    } else {
        home_dir()
            .join("Library/LaunchAgents")
            .join(format!("{}.plist", label(spec)))
    }
}

// MARK: env file → plist

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn valid_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Parse `KEY=value` lines out of the env file — blanks and `#` comments
/// are ignored, a missing file yields no vars (install still renders a
/// valid plist). Duplicate keys collapse to the last value.
fn parse_env_file(path: &Path) -> Vec<(String, String)> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut entries: Vec<(String, String)> = Vec::new();
    for line in raw.split('\n') {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some(eq) = trimmed.find('=') else {
            continue;
        };
        if eq == 0 {
            continue;
        }
        let key = trimmed[..eq].trim();
        if !valid_env_key(key) {
            continue;
        }
        let value = trimmed[eq + 1..].trim().to_string();
        // Duplicate keys collapse to the last value, position stable.
        match entries.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = value,
            None => entries.push((key.to_string(), value)),
        }
    }
    entries
}

/// Render the plist content — pure.
pub fn render(spec: &ServiceSpec) -> String {
    let env = parse_env_file(&spec.env_file);
    let args = spec
        .exec
        .iter()
        .map(|arg| format!("\t\t<string>{}</string>", xml_escape(arg)))
        .collect::<Vec<_>>()
        .join("\n");
    let env_block = if env.is_empty() {
        String::new()
    } else {
        let pairs = env
            .iter()
            .map(|(key, value)| {
                format!(
                    "\t\t<key>{}</key>\n\t\t<string>{}</string>",
                    xml_escape(key),
                    xml_escape(value)
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!("\t<key>EnvironmentVariables</key>\n\t<dict>\n{pairs}\n\t</dict>\n")
    };
    let log = xml_escape(&log_path(spec).to_string_lossy());
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \t<key>Label</key>\n\
         \t<string>{}</string>\n\
         \t<key>ProgramArguments</key>\n\
         \t<array>\n\
         {args}\n\
         \t</array>\n\
         {env_block}\t<key>RunAtLoad</key>\n\
         \t<true/>\n\
         \t<key>KeepAlive</key>\n\
         \t<dict>\n\
         \t\t<key>SuccessfulExit</key>\n\
         \t\t<false/>\n\
         \t</dict>\n\
         \t<key>StandardOutPath</key>\n\
         \t<string>{log}</string>\n\
         \t<key>StandardErrorPath</key>\n\
         \t<string>{log}</string>\n\
         </dict>\n\
         </plist>\n",
        label(spec)
    )
}

// MARK: launchctl

/// `bin` resolves on PATH — the install preflight's whole question.
fn on_path(bin: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|dir| {
            !dir.as_os_str().is_empty()
                && nix::unistd::access(&dir.join(bin), nix::unistd::AccessFlags::X_OK).is_ok()
        })
    })
}

fn require_launchctl() -> Result<(), CliError> {
    if !on_path("launchctl") {
        return Err(CliError(
            "launchd: `launchctl` not found on PATH — is this macOS?".into(),
        ));
    }
    Ok(())
}

struct Proc {
    code: i32,
    stdout: String,
    stderr: String,
}

/// One launchctl invocation, echoed before it runs (launchd logs go to
/// stdout — `console.log` in the TS port).
fn spawn(argv: &[&str]) -> Proc {
    println!("launchd: $ {}", argv.join(" "));
    let Some((program, args)) = argv.split_first() else {
        return Proc {
            code: 1,
            stdout: String::new(),
            stderr: "empty command".into(),
        };
    };
    match Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
    {
        Ok(out) => Proc {
            code: out.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        },
        Err(e) => Proc {
            code: 1,
            stdout: String::new(),
            stderr: e.to_string(),
        },
    }
}

/// Throwing variant — the command must succeed or the verb fails.
fn run(argv: &[&str]) -> Result<(), CliError> {
    let out = spawn(argv);
    if out.code != 0 {
        let detail = {
            let stderr = out.stderr.trim();
            if stderr.is_empty() {
                out.stdout.trim()
            } else {
                stderr
            }
        };
        return Err(CliError(format!(
            "launchd: `{}` exited {}: {}",
            argv.join(" "),
            out.code,
            detail
        )));
    }
    Ok(())
}

/// Whether launchd currently has the job loaded (`print` probes it).
fn loaded(spec: &ServiceSpec) -> bool {
    spawn(&["launchctl", "print", &service_target(spec)]).code == 0
}

/// Last `count` lines of a log file — "" when the file isn't there.
fn tail_lines(path: &Path, count: usize) -> String {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let mut lines: Vec<&str> = raw.split('\n').collect();
    while lines.last() == Some(&"") {
        lines.pop();
    }
    lines
        .iter()
        .skip(lines.len().saturating_sub(count))
        .copied()
        .collect::<Vec<_>>()
        .join("\n")
}

/// Write the env template (first install only — the file is user state,
/// a re-install only re-renders it into the plist), write the plist,
/// then bootstrap into the gui/system domain. Re-installing over a
/// loaded job bootouts first so the new plist actually takes.
pub fn install(spec: &ServiceSpec) -> Result<(), CliError> {
    require_launchctl()?;
    let plist = unit_path(spec);
    for dir in [
        plist.parent(),
        log_path(spec).parent(),
        spec.env_file.parent(),
    ]
    .into_iter()
    .flatten()
    {
        std::fs::create_dir_all(dir)
            .map_err(|e| CliError(format!("Failed to create {}: {e}", dir.display())))?;
    }
    if !spec.env_file.exists() {
        std::fs::write(&spec.env_file, env_template())
            .map_err(|e| CliError(format!("Failed to write {}: {e}", spec.env_file.display())))?;
    }
    std::fs::write(&plist, render(spec))
        .map_err(|e| CliError(format!("Failed to write {}: {e}", plist.display())))?;
    if loaded(spec) {
        let plist_str = plist.to_string_lossy().into_owned();
        let _ = spawn(&["launchctl", "bootout", &domain(spec), &plist_str]);
    }
    run(&[
        "launchctl",
        "bootstrap",
        &domain(spec),
        &plist.to_string_lossy(),
    ])
}

/// bootout is best-effort — "not loaded" isn't a failure to remove files.
pub fn uninstall(spec: &ServiceSpec, purge: bool) -> Result<(), CliError> {
    if on_path("launchctl") {
        let unit = unit_path(spec).to_string_lossy().into_owned();
        let _ = spawn(&["launchctl", "bootout", &domain(spec), &unit]);
    }
    let _ = std::fs::remove_file(unit_path(spec));
    if purge {
        let _ = std::fs::remove_file(&spec.env_file);
    }
    Ok(())
}

/// `launchctl print <domain>/<label>` is the modern state query — a
/// nonzero exit means the job isn't loaded. `installed` tracks launchd's
/// view; `enabled` tracks the plist file (presence = it loads at
/// login/boot).
pub fn status(spec: &ServiceSpec) -> Result<ServiceStatus, CliError> {
    let enabled = unit_path(spec).exists();
    let detail = tail_lines(&log_path(spec), STATUS_LOG_LINES);
    if !on_path("launchctl") {
        return Ok(ServiceStatus {
            installed: false,
            enabled,
            active: false,
            pid: None,
            detail,
        });
    }
    let out = spawn(&["launchctl", "print", &service_target(spec)]);
    if out.code != 0 {
        return Ok(ServiceStatus {
            installed: false,
            enabled,
            active: false,
            pid: None,
            detail,
        });
    }
    let field = |key: &str| {
        out.stdout.lines().find_map(|line| {
            let line = line.trim_start();
            line.strip_prefix(&format!("{key} = ")).map(str::trim)
        })
    };
    let state = field("state");
    let pid = field("pid").and_then(|v| v.parse::<u64>().ok());
    Ok(ServiceStatus {
        installed: true,
        enabled,
        active: state == Some("running") || pid.is_some_and(|p| p > 0),
        pid: pid.filter(|p| *p > 0),
        detail,
    })
}

/// kickstart -k = kill the running job and start it again on the new plist.
pub fn restart(spec: &ServiceSpec) -> Result<(), CliError> {
    require_launchctl()?;
    run(&["launchctl", "kickstart", "-k", &service_target(spec)])
}

/// Print the tail; `--follow` hands the terminal to `tail -f`.
pub fn logs(spec: &ServiceSpec, follow: bool) -> Result<i32, CliError> {
    let log = log_path(spec);
    if follow {
        let status = Command::new("tail")
            .args(["-n", &LOG_LINES.to_string(), "-f"])
            .arg(&log)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .map_err(|e| CliError(format!("launchd: `tail -f` failed to spawn: {e}")))?;
        return Ok(status.code().unwrap_or(0));
    }
    let tail = tail_lines(&log, LOG_LINES);
    if !tail.is_empty() {
        println!("{tail}");
    }
    Ok(0)
}

//! Port of `apps/sepia/src/service/systemd.ts` — per-user units at
//! `~/.config/systemd/user` (the default: no root, `EnvironmentFile` can
//! use the `%h` specifier) or system units at `/etc/systemd/system`.
//! `enable --now` starts the service on install and at boot/login;
//! `disable --now` stops it. `systemctl`/`journalctl` resolve via PATH
//! so tests can drive the backend against stub binaries.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use super::{CliError, SERVICE_NAME, ServiceSpec, ServiceStatus, env_template};

const UNIT: &str = "sepia.service";

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

/// systemd escapes ExecStart args by double-quoting (never single-quoting).
fn quote_arg(arg: &str) -> String {
    if arg
        .chars()
        .any(|c| c.is_whitespace() || c == '"' || c == '\'')
    {
        format!("\"{}\"", arg.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        arg.to_string()
    }
}

fn on_path(bin: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|path| {
        std::env::split_paths(&path).any(|dir| {
            !dir.as_os_str().is_empty()
                && nix::unistd::access(&dir.join(bin), nix::unistd::AccessFlags::X_OK).is_ok()
        })
    })
}

fn require_systemd(bin: &str) -> Result<(), CliError> {
    if !on_path(bin) {
        return Err(CliError(format!(
            "systemd not available — `{bin}` is not on PATH"
        )));
    }
    Ok(())
}

/// `systemctl` argv — `--user` for per-user units, plain for system units.
fn systemctl(spec: &ServiceSpec, args: &[&str]) -> Vec<String> {
    let mut cmd = vec!["systemctl".to_string()];
    if !spec.system {
        cmd.push("--user".into());
    }
    cmd.extend(args.iter().map(|a| (*a).to_string()));
    cmd
}

fn journalctl(spec: &ServiceSpec, args: &[&str]) -> Vec<String> {
    let mut cmd = vec!["journalctl".to_string()];
    if !spec.system {
        cmd.push("--user".into());
    }
    cmd.extend(args.iter().map(|a| (*a).to_string()));
    cmd
}

fn log_cmd(cmd: &[String]) {
    eprintln!("systemd: $ {}", cmd.join(" "));
}

struct Proc {
    code: i32,
    stdout: String,
    stderr: String,
}

fn spawn(cmd: &[String], stdout: Stdio, stderr: Stdio) -> Result<Proc, CliError> {
    let Some((program, args)) = cmd.split_first() else {
        return Err(CliError("empty command".into()));
    };
    let out = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .output()
        .map_err(|e| CliError(format!("`{}` failed to spawn: {e}", cmd.join(" "))))?;
    Ok(Proc {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}

/// Throwing variant — the command must succeed or the verb fails.
fn run(cmd: &[String]) -> Result<(), CliError> {
    log_cmd(cmd);
    let proc = spawn(cmd, Stdio::inherit(), Stdio::piped())?;
    if proc.code != 0 {
        let detail = proc.stderr.trim();
        return Err(CliError(format!(
            "`{}` exited {}{}",
            cmd.join(" "),
            proc.code,
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        )));
    }
    Ok(())
}

/// Same as `run` but tolerant — returns the exit code instead of throwing.
fn try_run(cmd: &[String]) -> i32 {
    log_cmd(cmd);
    spawn(cmd, Stdio::inherit(), Stdio::piped()).map_or(1, |p| p.code)
}

fn capture(cmd: &[String]) -> Proc {
    spawn(cmd, Stdio::piped(), Stdio::piped()).unwrap_or(Proc {
        code: 1,
        stdout: String::new(),
        stderr: String::new(),
    })
}

/// Where the unit file lives for this spec.
pub fn unit_path(spec: &ServiceSpec) -> PathBuf {
    if spec.system {
        PathBuf::from(format!("/etc/systemd/system/{UNIT}"))
    } else {
        home_dir().join(".config/systemd/user").join(UNIT)
    }
}

/// Render the unit content — pure.
pub fn render(spec: &ServiceSpec) -> String {
    // User units may use the %h specifier (the service manager resolves
    // it); system units can't — a literal absolute path is required there.
    let home = home_dir();
    let home_str = home.to_string_lossy();
    let env_str = spec.env_file.to_string_lossy();
    let env_file = if !spec.system && env_str.starts_with(&format!("{home_str}/")) {
        format!("%h{}", &env_str[home_str.len()..])
    } else {
        env_str.into_owned()
    };
    format!(
        "[Unit]\n\
         Description=Sepia node\n\
         After=network-online.target\n\
         \n\
         [Service]\n\
         ExecStart={}\n\
         EnvironmentFile=-{env_file}\n\
         Restart=on-failure\n\
         RestartSec=2\n\
         \n\
         [Install]\n\
         WantedBy={}\n",
        spec.exec
            .iter()
            .map(|a| quote_arg(a))
            .collect::<Vec<_>>()
            .join(" "),
        if spec.system {
            "multi-user.target"
        } else {
            "default.target"
        },
    )
}

/// Write unit + env template, reload, enable + start. Idempotent.
pub fn install(spec: &ServiceSpec) -> Result<(), CliError> {
    require_systemd("systemctl")?;
    let unit = unit_path(spec);
    if let Some(dir) = unit.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| CliError(format!("Failed to create {}: {e}", dir.display())))?;
    }
    std::fs::write(&unit, render(spec))
        .map_err(|e| CliError(format!("Failed to write {}: {e}", unit.display())))?;
    // The env template is written once — never clobber an admin's edits.
    if !spec.env_file.exists() {
        if let Some(dir) = spec.env_file.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| CliError(format!("Failed to create {}: {e}", dir.display())))?;
        }
        std::fs::write(&spec.env_file, env_template())
            .map_err(|e| CliError(format!("Failed to write {}: {e}", spec.env_file.display())))?;
    }
    run(&systemctl(spec, &["daemon-reload"]))?;
    // A prior crash loop trips the start limit — clear it or the enable
    // refuses with 'Unit ... failed' even though the unit is healthy now.
    let _ = try_run(&systemctl(spec, &["reset-failed", SERVICE_NAME]));
    run(&systemctl(spec, &["enable", "--now", SERVICE_NAME]))
}

/// Stop, disable, remove the unit. `purge` drops the env file too.
pub fn uninstall(spec: &ServiceSpec, purge: bool) -> Result<(), CliError> {
    require_systemd("systemctl")?;
    // Tolerate a missing/partial unit — removing the file is the source
    // of truth.
    let _ = try_run(&systemctl(spec, &["disable", "--now", SERVICE_NAME]));
    let unit = unit_path(spec);
    let _ = std::fs::remove_file(&unit);
    run(&systemctl(spec, &["daemon-reload"]))?;
    if purge {
        let _ = std::fs::remove_file(&spec.env_file);
    }
    Ok(())
}

// `is-enabled` prints one of these to stdout when the unit is known; a
// missing unit exits non-zero with nothing on stdout.
const KNOWN_UNIT_STATES: [&str; 9] = [
    "enabled",
    "linked",
    "static",
    "indirect",
    "generated",
    "transient",
    "alias",
    "masked",
    "disabled",
];

const NOT_INSTALLED: ServiceStatus = ServiceStatus {
    installed: false,
    enabled: false,
    active: false,
    pid: None,
    detail: String::new(),
};

pub fn status(spec: &ServiceSpec) -> Result<ServiceStatus, CliError> {
    if !on_path("systemctl") {
        return Ok(NOT_INSTALLED);
    }
    let enabled_proc = capture(&systemctl(spec, &["is-enabled", SERVICE_NAME]));
    let enabled_state = enabled_proc.stdout.trim().to_string();
    let installed = unit_path(spec).exists()
        || KNOWN_UNIT_STATES
            .iter()
            .any(|state| enabled_state.starts_with(state));
    if !installed {
        return Ok(NOT_INSTALLED);
    }
    let active_proc = capture(&systemctl(spec, &["is-active", SERVICE_NAME]));
    let pid_proc = capture(&systemctl(spec, &["show", SERVICE_NAME, "-p", "MainPID"]));
    let pid = pid_proc
        .stdout
        .lines()
        .find_map(|line| line.strip_prefix("MainPID="))
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|pid| *pid > 0);
    let detail = if on_path("journalctl") {
        capture(&journalctl(
            spec,
            &["-u", SERVICE_NAME, "-n", "10", "--no-pager"],
        ))
        .stdout
        .trim()
        .to_string()
    } else {
        String::new()
    };
    Ok(ServiceStatus {
        installed: true,
        enabled: enabled_state.starts_with("enabled"),
        active: active_proc.stdout.trim() == "active",
        pid,
        detail,
    })
}

pub fn restart(spec: &ServiceSpec) -> Result<(), CliError> {
    require_systemd("systemctl")?;
    run(&systemctl(spec, &["restart", SERVICE_NAME]))
}

/// `-f` hands the terminal to `journalctl -f`; returns its exit code.
pub fn logs(spec: &ServiceSpec, follow: bool) -> Result<i32, CliError> {
    require_systemd("journalctl")?;
    let mut args = vec!["-u", SERVICE_NAME];
    if follow {
        args.push("-f");
    }
    let cmd = journalctl(spec, &args);
    log_cmd(&cmd);
    let Some((program, rest)) = cmd.split_first() else {
        return Err(CliError("empty command".into()));
    };
    let status = Command::new(program)
        .args(rest)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| CliError(format!("`{}` failed to spawn: {e}", cmd.join(" "))))?;
    Ok(status.code().unwrap_or(-1))
}

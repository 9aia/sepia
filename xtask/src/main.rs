use std::process::{Command, Stdio};

fn run(cmd: &str, args: &[&str]) -> anyhow::Result<()> {
    let status = Command::new(cmd).args(args).status()?;
    anyhow::ensure!(status.success(), "{cmd} {args:?} failed");
    Ok(())
}

/// True when the `cargo-nextest` subcommand is installed. Probed up
/// front so a *failing* nextest run reports its own failure instead of
/// silently re-running the whole suite a second time under `cargo test`.
fn nextest_available() -> bool {
    match Command::new("cargo")
        .args(["nextest", "--version"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(status) => status.success(),
        Err(_) => false,
    }
}

fn main() -> anyhow::Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some("check") => {
            // clippy --all-targets type-checks every target already —
            // a separate `cargo check` pass would redo that work.
            run("cargo", &["fmt", "--all", "--check"])?;
            run(
                "cargo",
                &[
                    "clippy",
                    "--workspace",
                    "--all-targets",
                    "--",
                    "-D",
                    "warnings",
                ],
            )?;
        }
        // Prefer nextest when installed, but only fall back to
        // `cargo test` when the subcommand is *missing* — a failing
        // nextest run must not re-run the whole suite a second way.
        Some("test") => {
            // Prebuild the binaries tests shell out to (driver, mock
            // ACP agent, the sepia CLI itself) — one cargo invocation
            // here beats N in-test `cargo build`s serializing on
            // package/build-dir locks. Tests see SEPIA_TESTKIT_PREBUILT
            // and resolve the paths directly.
            run(
                "cargo",
                &[
                    "build",
                    "--bin",
                    "sepia",
                    "--bin",
                    "sepia-driver-devin",
                    "--bin",
                    "sepia-mock-acp-agent",
                ],
            )?;
            let cmd = if nextest_available() {
                &["nextest", "run", "--workspace"][..]
            } else {
                &["test", "--workspace"][..]
            };
            let status = Command::new("cargo")
                .args(cmd)
                .env("SEPIA_TESTKIT_PREBUILT", "1")
                .status()?;
            anyhow::ensure!(status.success(), "workspace tests failed");
        }
        Some("install") => {
            // Build every user-facing binary in release and copy it to
            // ~/.local/bin (idempotent, PATH-friendly names).
            let bins = [
                "sepia",
                "sepia-driver-devin",
                "sepia-driver-cline",
                "sepia-driver-claude",
                "sepia-driver-cursor",
            ];
            // One invocation for all binaries: a single resolve and
            // fingerprint pass instead of several.
            let mut build = vec!["build", "--release"];
            for bin in &bins {
                build.push("--bin");
                build.push(*bin);
            }
            run("cargo", &build)?;
            let home = std::env::var_os("HOME")
                .map_or_else(|| std::path::PathBuf::from("/"), std::path::PathBuf::from);
            let out = home.join(".local/bin");
            std::fs::create_dir_all(&out)?;
            for bin in &bins {
                std::fs::copy(format!("target/release/{bin}"), out.join(bin))?;
            }
            println!("installed {bins:?} to {}", out.display());

            // Driver discovery searches `$SEPIA_DRIVER_DIR` →
            // `~/.local/share/sepia/drivers` → `/usr/local/lib/sepia/
            // drivers` → PATH. Under systemd PATH never includes
            // `~/.local/bin`, so link the drivers into the canonical
            // dir — without this a spawned plane sees zero drivers and
            // every store op misses.
            let driver_dir = home.join(".local/share/sepia/drivers");
            std::fs::create_dir_all(&driver_dir)?;
            for bin in &bins {
                if !bin.starts_with("sepia-driver-") {
                    continue;
                }
                let link = driver_dir.join(bin);
                let _ = std::fs::remove_file(&link);
                #[cfg(unix)]
                std::os::unix::fs::symlink(out.join(bin), &link)?;
                #[cfg(not(unix))]
                std::fs::copy(out.join(bin), &link)?;
            }
            println!("linked drivers into {}", driver_dir.display());
        }
        Some(other) => anyhow::bail!("unknown task {other}"),
        None => {
            anyhow::bail!("usage: cargo xtask <check|test|install>")
        }
    }
    Ok(())
}

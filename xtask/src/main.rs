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

/// True when the `cargo-llvm-cov` subcommand is installed.
fn llvm_cov_available() -> bool {
    Command::new("cargo")
        .args(["llvm-cov", "--version"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Binaries tests shell out to (drivers, mock ACP agent, the engine
/// driver the `sepia` shim resolves to) — one `cargo build` beats N
/// in-test builds serializing on package/build-dir locks. Tests see
/// SEPIA_TESTKIT_PREBUILT and resolve the paths directly.
const TEST_BINS: &[&str] = &[
    "tamias-driver-sepia",
    "sepia-driver-devin",
    "sepia-driver-cline",
    "sepia-driver-claude",
    "sepia-driver-cursor",
    "sepia-driver-memtest",
    "sepia-mock-acp-agent",
];

/// The effective target dir — `cargo metadata` reports it, which keeps
/// `install` correct under a shared CARGO_TARGET_DIR (`target/release`
/// relative to the workspace is stale).
fn target_dir() -> anyhow::Result<std::path::PathBuf> {
    let out = Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .output()?;
    let meta: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    meta.get("target_directory")
        .and_then(|v| v.as_str())
        .map(std::path::PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("cargo metadata: no target_directory"))
}

fn prebuild() -> anyhow::Result<()> {
    let mut args = vec!["build"];
    for bin in TEST_BINS {
        args.push("--bin");
        args.push(bin);
    }
    run("cargo", &args)
}

/// Captured stdout of a command — for probes like `rustc -vV`.
fn run_out(args: &[&str]) -> Option<String> {
    let (prog, rest) = args.split_first()?;
    let out = Command::new(prog).args(rest).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The host target triple — the `drivers/bin/<triple>/` selector
/// tamias uses for compiled driver payloads.
fn host_triple() -> String {
    run_out(&["rustc", "-vV"])
        .and_then(|v| {
            v.lines()
                .find_map(|l| l.strip_prefix("host: ").map(str::to_string))
        })
        .unwrap_or_else(|| "x86_64-unknown-linux-gnu".to_string())
}

/// `cargo xtask pack` — stage the tamias pack payload (T-012):
/// release-build `tamias-driver-sepia` and drop it under
/// `libraries/sepia/drivers/bin/<host-triple>/` — the
/// bundled-driver dir `tamias install` registers into
/// `<tamias-home>/drivers/`. Gitignored on purpose: the binary is a
/// build artifact; the committed pack is just `tamias.yaml` + this
/// staging verb.
fn pack() -> anyhow::Result<()> {
    run("cargo", &["build", "--release", "-p", "sepia-driver-pavo"])?;
    let triple = host_triple();
    let dest_dir = std::path::PathBuf::from("libraries/sepia/drivers/bin").join(&triple);
    std::fs::create_dir_all(&dest_dir)?;
    let src = std::path::PathBuf::from("target/release/tamias-driver-sepia");
    let dest = dest_dir.join("tamias-driver-sepia");
    std::fs::copy(&src, &dest)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755));
    }
    println!("pack staged: {}", dest.display());
    println!();
    println!("install the sepia command (generated bins[] shim):");
    println!("  tamias install --allow-drivers ./libraries/sepia");
    println!("  # or a pinned tarball: tamias bundle pack libraries/sepia");
    Ok(())
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
            prebuild()?;
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
        Some("pack") => pack()?,
        Some("install") => {
            // Build every user-facing binary in release and copy it to
            // ~/.local/bin (idempotent, PATH-friendly names). There is
            // no `sepia` cargo bin — the command ships as the tamias
            // pack (`cargo xtask pack` + `tamias install
            // ./libraries/sepia`); this installs the driver the shim
            // resolves plus the store drivers.
            let bins = [
                "tamias-driver-sepia",
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
            let target = target_dir()?;
            let home = std::env::var_os("HOME")
                .map_or_else(|| std::path::PathBuf::from("/"), std::path::PathBuf::from);
            let out = home.join(".local/bin");
            std::fs::create_dir_all(&out)?;
            for bin in &bins {
                let dst = out.join(bin);
                // Atomic replace — a running driver holds the old inode
                // busy ("text file busy" on plain copy).
                std::fs::copy(
                    target.join("release").join(bin),
                    out.join(format!(".{bin}.new")),
                )?;
                std::fs::rename(out.join(format!(".{bin}.new")), &dst)?;
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
        // `cargo xtask coverage [llvm-cov args…]` — HTML report under
        // target/llvm-cov/html plus a printed summary. Extra args pass
        // through to cargo-llvm-cov (e.g. `--fail-under-lines 100`,
        // `--lcov --output-path lcov.info` for CI upload); passing any
        // report flag replaces the default --html.
        Some("coverage") => {
            anyhow::ensure!(
                llvm_cov_available(),
                "cargo-llvm-cov not installed — `cargo install cargo-llvm-cov` \
                 (needs the llvm-tools rustup component)"
            );
            // Prebuild the spawned binaries *inside* the coverage env:
            // llvm-cov's rustc wrapper instruments only the workspace
            // crates named at show-env time, so bins built outside it
            // run uninstrumented and e2e coverage stays invisible.
            let mut bins = String::new();
            for bin in TEST_BINS {
                bins.push_str(" --bin ");
                bins.push_str(bin);
            }
            let pre = format!(
                "eval \"$(cargo llvm-cov show-env --export-prefix)\" \
                 && cargo build{bins}"
            );
            run("sh", &["-c", &pre])?;
            // --no-tests=pass matches `xtask test`: an empty suite is
            // a pass, not nextest's exit-4 "no tests to run".
            let runner = if nextest_available() {
                "cargo llvm-cov nextest --workspace --exclude xtask --no-tests=pass"
            } else {
                "cargo llvm-cov --workspace --exclude xtask"
            };
            let extra: Vec<String> = std::env::args().skip(2).collect();
            let wants_report = extra.iter().any(|a| {
                matches!(
                    a.as_str(),
                    "--text"
                        | "--summary-only"
                        | "--json"
                        | "--lcov"
                        | "--cobertura"
                        | "--codecov"
                        | "--open"
                )
            });
            let html = if wants_report { "" } else { " --html" };
            let cov = format!("{runner}{html} \"$@\"");
            // Spawned test binaries must resolve to the *instrumented*
            // ones — llvm-cov builds into <target>/llvm-cov-target, so
            // point testkit's search there ahead of the plain debug dir.
            let instrumented_bins = target_dir()?.join("llvm-cov-target/debug");
            let status = Command::new("sh")
                .args(["-c", &cov, "xtask-coverage"])
                .args(&extra)
                .env("SEPIA_TESTKIT_PREBUILT", "1")
                .env("TESTKIT_BIN_DIRS", &instrumented_bins)
                .status()?;
            anyhow::ensure!(status.success(), "coverage run failed");
        }
        Some(other) => anyhow::bail!("unknown task {other}"),
        None => {
            anyhow::bail!("usage: cargo xtask <check|test|pack|install|coverage>")
        }
    }
    Ok(())
}

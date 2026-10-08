use std::process::Command;

fn run(cmd: &str, args: &[&str]) -> anyhow::Result<()> {
    let status = Command::new(cmd).args(args).status()?;
    anyhow::ensure!(status.success(), "{cmd} {args:?} failed");
    Ok(())
}

fn main() -> anyhow::Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some("check") => {
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
            run("cargo", &["check", "--workspace", "--all-targets"])?;
        }
        Some("test") => run("cargo", &["nextest", "run", "--workspace"])
            .or_else(|_| run("cargo", &["test", "--workspace"]))?,
        Some("install") => {
            // Build every user-facing binary in release and copy it to
            // ~/.local/bin (idempotent, PATH-friendly names).
            let bins = [
                "sepia",
                "sepia-node",
                "sepia-driver-devin",
                "sepia-driver-cline",
                "sepia-driver-claude",
                "sepia-driver-cursor",
                "sepia-hub",
            ];
            for bin in &bins {
                run("cargo", &["build", "--release", "--bin", bin])?;
            }
            let home = std::env::var_os("HOME")
                .map_or_else(|| std::path::PathBuf::from("/"), std::path::PathBuf::from);
            let out = home.join(".local/bin");
            std::fs::create_dir_all(&out)?;
            for bin in &bins {
                std::fs::copy(format!("target/release/{bin}"), out.join(bin))?;
            }
            println!("installed {bins:?} to {}", out.display());
        }
        Some(other) => anyhow::bail!("unknown task {other}"),
        None => anyhow::bail!("usage: cargo xtask <check|test|install>"),
    }
    Ok(())
}

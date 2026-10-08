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
        Some(other) => anyhow::bail!("unknown task {other}"),
        None => anyhow::bail!("usage: cargo xtask <check|test>"),
    }
    Ok(())
}

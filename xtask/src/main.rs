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

            // The hub's wasm bundle + PWA assets — hydration JS lives in
            // $SEPIA_HOME/site (LEPTOS_SITE_ROOT/SEPIA_SITE_ROOT override).
            build_site()?;
        }
        Some("site") => build_site()?,
        Some(other) => anyhow::bail!("unknown task {other}"),
        None => anyhow::bail!("usage: cargo xtask <check|test|install|site>"),
    }
    Ok(())
}

/// `cargo xtask site` — build the Leptos wasm bundle + stage the PWA
/// assets into `target/site` (pkg/, manifest.json, sw.js, icon.svg),
/// then copy to `~/.local/share/sepia/site` for `sepia-hub`'s default
/// site root. Requires `wasm-bindgen-cli` — its version must match the
/// locked `wasm-bindgen` crate (`cargo metadata`-checked).
fn build_site() -> anyhow::Result<()> {
    let site = std::path::Path::new("target/site");
    std::fs::create_dir_all(site.join("pkg"))?;
    run(
        "cargo",
        &[
            "build",
            "--release",
            "--package",
            "sepia-web",
            "--target",
            "wasm32-unknown-unknown",
            "--features",
            "hydrate",
        ],
    )?;
    run(
        "wasm-bindgen",
        &[
            "--target",
            "web",
            "--out-dir",
            "target/site/pkg",
            "target/wasm32-unknown-unknown/release/sepia_web.wasm",
        ],
    )?;
    // PWA assets ship in the crate's public/ dir.
    for file in ["manifest.json", "sw.js", "icon.svg"] {
        let src = format!("crates/sepia-hub/public/{file}");
        if std::path::Path::new(&src).exists() {
            std::fs::copy(&src, site.join(file))?;
        }
    }
    let home = std::env::var_os("SEPIA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share/sepia"))
        })
        .unwrap_or_else(|| std::path::PathBuf::from(".sepia"));
    let dst = home.join("site");
    std::fs::create_dir_all(&dst)?;
    copy_dir(site, &dst)?;
    println!("site staged to {}", dst.display());
    Ok(())
}

/// Recursively copy a directory (small site dirs — no hardlink games).
fn copy_dir(src: &std::path::Path, dst: &std::path::Path) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            std::fs::create_dir_all(&target)?;
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

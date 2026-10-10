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
            check_cfg_never_gates_markup()?;
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
            if nextest_available() {
                run("cargo", &["nextest", "run", "--workspace"])?;
            } else {
                run("cargo", &["test", "--workspace"])?;
            }
        }
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
            // One invocation for all binaries: a single resolve and
            // fingerprint pass instead of seven.
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

            // The hub's wasm bundle + PWA assets — hydration JS lives in
            // $SEPIA_HOME/site (LEPTOS_SITE_ROOT/SEPIA_SITE_ROOT override).
            build_site()?;
        }
        Some("site") => build_site()?,
        // Headless-Chrome console probe against the live stack
        // (default http://localhost:3000). The UI regression gate —
        // run after any sepia-web change.
        Some("smoke") => {
            let args: Vec<String> = std::env::args().skip(2).collect();
            let status = Command::new("python3")
                .arg("tools/browser-smoke.py")
                .args(&args)
                .status()?;
            anyhow::ensure!(status.success(), "browser smoke failed");
        }
        // Browser e2e — prebuild the binaries once so the tests
        // resolve paths instead of each shelling `cargo build`, then
        // run the ignored suite under nextest (or cargo test).
        Some("e2e") => {
            run(
                "cargo",
                &[
                    "build",
                    "--bin",
                    "sepia-driver-devin",
                    "--bin",
                    "sepia-node",
                    "--bin",
                    "sepia-hub",
                ],
            )?;
            anyhow::ensure!(
                std::path::Path::new("target/site/pkg").is_dir(),
                "target/site/pkg missing — run `cargo xtask site` first"
            );
            // PREBUILT tells the harness the binaries are fresh —
            // warm_binaries then skips its cargo invocation entirely.
            let env = [("SEPIA_BROWSER_E2E", "1"), ("SEPIA_E2E_PREBUILT", "1")];
            if nextest_available() {
                // Fast signal first: `journey` walks the whole critical
                // path in one shared env+browser, so a regression fails
                // in minutes instead of after the serialized suite.
                let journey = Command::new("cargo")
                    .args([
                        "nextest",
                        "run",
                        "-p",
                        "sepia-hub",
                        "--test",
                        "e2e",
                        "--run-ignored",
                        "all",
                        "-E",
                        "test(journey)",
                    ])
                    .envs(env)
                    .status()?;
                anyhow::ensure!(journey.success(), "journey e2e failed");
                let status = Command::new("cargo")
                    .args([
                        "nextest",
                        "run",
                        "-p",
                        "sepia-hub",
                        "--test",
                        "e2e",
                        "--run-ignored",
                        "all",
                    ])
                    .envs(env)
                    .status()?;
                anyhow::ensure!(status.success(), "browser e2e failed");
            } else {
                // `cargo test` has no filtersets — the positional
                // substring filter selects the journey, then all.
                let journey = Command::new("cargo")
                    .args([
                        "test",
                        "-p",
                        "sepia-hub",
                        "--test",
                        "e2e",
                        "journey",
                        "--",
                        "--ignored",
                    ])
                    .envs(env)
                    .status()?;
                anyhow::ensure!(journey.success(), "journey e2e failed");
                let status = Command::new("cargo")
                    .args([
                        "test",
                        "-p",
                        "sepia-hub",
                        "--test",
                        "e2e",
                        "--",
                        "--ignored",
                    ])
                    .envs(env)
                    .status()?;
                anyhow::ensure!(status.success(), "browser e2e failed");
            }
        }
        Some(other) => anyhow::bail!("unknown task {other}"),
        None => {
            anyhow::bail!("usage: cargo xtask <check|test|install|site|smoke|e2e>")
        }
    }
    Ok(())
}

/// `cargo xtask site` — build the Leptos wasm bundle + stage the PWA
/// assets into `target/site` (pkg/, manifest.json, sw.js, icon.svg),
/// then copy to `~/.local/share/sepia/site` for `sepia-hub`'s default
/// site root. Requires `wasm-bindgen-cli` — its version must match the
/// locked `wasm-bindgen` crate (`cargo metadata`-checked).
fn build_site() -> anyhow::Result<()> {
    // Tailwind v4 → crates/sepia-web/style/main.css (embedded via
    // include_str!). Needs node_modules installed (`bun install` /
    // `npm i`); uses whatever `tailwindcss` binary is on PATH via bunx
    // or npx — checked in that order.
    let tw = ["bun", "x", "tailwindcss"];
    let tw_npm = ["npx", "tailwindcss"];
    let (tool, pre) = if which_available(tw[0]) {
        (&tw[..1], &tw[1..])
    } else if which_available(tw_npm[0]) {
        (&tw_npm[..1], &tw_npm[1..])
    } else {
        anyhow::bail!(
            "tailwind css build needs bun or npx on PATH              (install deps with `bun install` in the repo root)"
        );
    };
    let mut args: Vec<&str> = pre.to_vec();
    args.extend([
        "-i",
        "crates/sepia-web/style/input.css",
        "-o",
        "crates/sepia-web/style/main.css",
        "--minify",
    ]);
    run(tool[0], &args)?;

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
    // Content-hash the bundle stem (`sepia_web_<hash>`) so a stale
    // service-worker/browser cache can never hydrate old wasm against
    // new SSR HTML. The hub discovers the stem by scanning pkg/.
    let wasm = site.join("pkg/sepia_web_bg.wasm");
    let hash = {
        use sha2::Digest;
        let bytes = std::fs::read(&wasm)?;
        let digest = sha2::Sha256::digest(&bytes);
        digest[..4]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    let stem = format!("sepia_web_{hash}");
    let js = site.join("pkg/sepia_web.js");
    let js_hashed = site.join(format!("pkg/{stem}.js"));
    let wasm_hashed = site.join(format!("pkg/{stem}_bg.wasm"));
    // The loader fetches `<stem>_bg.wasm` relative to itself.
    let js_src =
        std::fs::read_to_string(&js)?.replace("sepia_web_bg.wasm", &format!("{stem}_bg.wasm"));
    std::fs::write(&js_hashed, js_src)?;
    std::fs::rename(&wasm, &wasm_hashed)?;
    std::fs::remove_file(&js)?;
    if let Ok(d) = std::fs::read_dir(site.join("pkg")) {
        for f in d.flatten() {
            let name = f.file_name().to_string_lossy().into_owned();
            if name.starts_with("sepia_web")
                && name != format!("{stem}.js")
                && name != format!("{stem}_bg.wasm")
            {
                let _ = std::fs::remove_file(f.path());
            }
        }
    }
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
    // pkg/ is bundle-only — wipe it so stale hashed stems don't pile up.
    let _ = std::fs::remove_dir_all(dst.join("pkg"));
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

/// `check` gate: `#[cfg]` may gate *behavior* (listeners, storage,
/// timers), never markup shape — a cfg'd `view!`/`impl IntoView` pair
/// is exactly the divergence class behind the ThemeToggle hydration
/// panic (SSR and hydrate silently emitting different nodes). The scan
/// is a plain line pass over `crates/sepia-web/src`: any cfg attr
/// mentioning `feature = "ssr"`/`"hydrate"` followed within a few
/// lines by `view!` or `impl IntoView` fails. Whole-file gating stays
/// clean by construction — the cfg sits on the `mod` decl (e.g.
/// `shell.rs`), so the markup inside it is unconditional.
fn check_cfg_never_gates_markup() -> anyhow::Result<()> {
    /// Lines after the attribute to inspect — enough for an
    /// intervening `#[component]` + signature, short enough not to
    /// bleed into the next item.
    const LOOKAHEAD: usize = 8;
    let mut files = Vec::new();
    rs_files(std::path::Path::new("crates/sepia-web/src"), &mut files)?;
    let mut violations = Vec::new();
    for path in files {
        let text = std::fs::read_to_string(&path)?;
        let lines: Vec<&str> = text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let t = line.trim_start();
            if !(t.starts_with("#[cfg(")
                && (t.contains(r#"feature = "ssr""#) || t.contains(r#"feature = "hydrate""#)))
            {
                continue;
            }
            for (j, next) in lines.iter().enumerate().skip(i + 1).take(LOOKAHEAD) {
                let n = next.trim_start();
                if n.starts_with("//") {
                    continue;
                }
                if n.contains("view!") || (n.contains("impl ") && n.contains("IntoView")) {
                    violations.push(format!(
                        "  {}:{} — `{t}` → `{}` (line {})",
                        path.display(),
                        i + 1,
                        n,
                        j + 1
                    ));
                    break;
                }
            }
        }
    }
    anyhow::ensure!(
        violations.is_empty(),
        "cfg must gate behavior, never markup shape (view!/impl IntoView):\n{}",
        violations.join("\n")
    );
    Ok(())
}

fn rs_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            rs_files(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    Ok(())
}

fn which_available(bin: &str) -> bool {
    std::process::Command::new(bin)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

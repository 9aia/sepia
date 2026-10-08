//! `sepia pair` — the minted code's shape and the `pair-code` file the
//! server consumes.

#![allow(clippy::unwrap_used, clippy::pedantic)]

use std::process::{Command, Output};

use tempfile::TempDir;

fn sepia(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sepia"))
        .args(args)
        .output()
        .unwrap()
}

/// Crockford base32 — no I/L/O/U, `XXXX-XXXX` (~40 bits).
fn is_crockford_code(code: &str) -> bool {
    let alphabet = |c: char| {
        c.is_ascii_digit() || matches!(c, 'A'..='H' | 'J' | 'K' | 'M' | 'N' | 'P'..='T' | 'V'..='Z')
    };
    let (a, b) = code.split_once('-').unwrap_or(("", ""));
    code.len() == 9
        && a.len() == 4
        && b.len() == 4
        && a.chars().all(alphabet)
        && b.chars().all(alphabet)
}

#[test]
fn mint_pair_code_shape() {
    for _ in 0..50 {
        let code = sepia_http::pair::mint_pair_code();
        assert!(is_crockford_code(&code), "bad code shape: {code}");
    }
}

#[test]
fn pair_writes_the_code_file_and_prints_it() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("sepia-home");
    let home_str = home.to_string_lossy().into_owned();

    let out = sepia(&["pair", "--home", &home_str, "--url", "http://node:9999"]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Pairing code (valid 60s, single use):"),
        "{stdout}"
    );
    assert!(stdout.contains("Node URL: http://node:9999"), "{stdout}");

    // The printed code is the standalone `XXXX-XXXX` line.
    let code = stdout
        .lines()
        .map(str::trim)
        .find(|line| is_crockford_code(line))
        .unwrap_or_else(|| panic!("no Crockford code in output:\n{stdout}"));

    // $SEPIA_HOME/pair-code carries {code, expiresAt} for the server.
    let raw = std::fs::read_to_string(home.join("pair-code")).unwrap();
    let file: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(file["code"].as_str().unwrap(), code);
    let expires_at = file["expiresAt"].as_u64().unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    assert!(expires_at > now, "code already expired");
    assert!(expires_at <= now + 61_000, "ttl beyond the 60s window");

    // 0600 — only the owner reads the mint file.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(home.join("pair-code"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "pair-code mode was {mode:o}");
    }
}

#[test]
fn pair_codes_are_unique() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join("sepia-home");
    let home_str = home.to_string_lossy().into_owned();
    let mut codes = std::collections::HashSet::new();
    for _ in 0..5 {
        let out = sepia(&["pair", "--home", &home_str]);
        let stdout = String::from_utf8_lossy(&out.stdout);
        let code = stdout
            .lines()
            .map(str::trim)
            .find(|line| is_crockford_code(line))
            .unwrap();
        assert!(codes.insert(code.to_string()), "duplicate code {code}");
    }
}

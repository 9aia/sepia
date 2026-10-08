//! Port of `apps/sepia/src/pair.ts` — the mint half of pairing
//! (docs/protocol.md "Auth — pairing"): `sepia pair` runs on the node
//! itself and writes `$SEPIA_HOME/pair-code` — `{code, expiresAt}` —
//! which the running server consumes on the next `POST /api/pair`. The
//! filesystem is the gate: only someone who can write `$SEPIA_HOME` (the
//! machine owner) can mint a code. The file format and code alphabet
//! live in `sepia-http::pair` — keep them in sync.

use std::path::{Path, PathBuf};

use crate::CliError;

pub use sepia_http::pair::PAIR_CODE_TTL_MS;
pub use sepia_http::pair::mint_pair_code;

/// What the mint produced — `path` is for messages, never logged verbatim.
pub struct MintedPairCode {
    pub code: String,
    pub expires_at: u64,
    pub path: PathBuf,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Atomically write the code file (0600, tmp+rename) so the server never
/// reads a half-written or world-readable code.
pub fn write_pair_code_file(home: &Path) -> Result<MintedPairCode, CliError> {
    let code = mint_pair_code();
    let expires_at = now_ms() + PAIR_CODE_TTL_MS;
    std::fs::create_dir_all(home)
        .map_err(|e| CliError(format!("Failed to create {}: {e}", home.display())))?;
    let path = home.join("pair-code");
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    let body = serde_json::json!({ "code": code, "expiresAt": expires_at });
    std::fs::write(&tmp, body.to_string())
        .map_err(|e| CliError(format!("Failed to write {}: {e}", tmp.display())))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| CliError(format!("Failed to chmod {}: {e}", tmp.display())))?;
    }
    std::fs::rename(&tmp, &path)
        .map_err(|e| CliError(format!("Failed to write {}: {e}", path.display())))?;
    Ok(MintedPairCode {
        code,
        expires_at,
        path,
    })
}

/// `sepia pair` — print a one-time pairing code; the UI exchanges it via
/// `POST /api/pair`.
pub fn pair(home: &Path, url: &str) -> Result<(), CliError> {
    // Mint = write the code file the running server consumes; whoever can
    // write $SEPIA_HOME is the machine owner, which is the whole gate.
    let minted = write_pair_code_file(home)?;
    println!(
        "Pairing code (valid {}s, single use):",
        PAIR_CODE_TTL_MS.div_ceil(1000)
    );
    println!("\n  {}\n", minted.code);
    println!("Node URL: {url}");
    println!("Enter both in Settings → Nodes → \"Pair with code\" before it expires.");
    Ok(())
}

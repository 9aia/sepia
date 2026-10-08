//! Pairing — port of `apps/server/src/pair.ts`: the one-time short code
//! `sepia pair` writes to `$SEPIA_HOME/pair-code` redeems for a long-lived
//! `sepia_…` bearer credential. Issued credentials persist as sha256
//! hashes in `$SEPIA_HOME/tokens.json` — the file alone can't
//! authenticate.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

/// How long a minted code stays redeemable (docs/protocol.md: ~60s).
pub const PAIR_CODE_TTL_MS: u64 = 60_000;

// Crockford base32 — no I/L/O/U, so codes survive being read aloud.
const CROCKFORD: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn random_bytes(n: usize) -> Vec<u8> {
    // uuid v4 is already backed by the OS CSPRNG — 16 bytes per call.
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        out.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    }
    out.truncate(n);
    out
}

/// 8 Crockford chars in a 4-4 group, e.g. `7K2M-9PQX` (~40 bits).
pub fn mint_pair_code() -> String {
    let bytes = random_bytes(8);
    let chars: String = bytes
        .iter()
        .map(|b| CROCKFORD[usize::from(b % u8::try_from(CROCKFORD.len()).unwrap_or(32))] as char)
        .collect();
    format!("{}-{}", &chars[..4], &chars[4..])
}

/// User input → lookup key: case-insensitive, dashes/spaces optional.
pub fn normalize_pair_code(input: &str) -> String {
    input
        .to_uppercase()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect()
}

/// The long-lived credential handed out on redeem.
pub fn mint_pair_token() -> String {
    format!("sepia_{}", URL_SAFE_NO_PAD.encode(random_bytes(32)))
}

/// Tokens persist as hashes only — the file alone can't authenticate.
pub fn hash_pair_token(token: &str) -> String {
    use std::fmt::Write;
    let digest = Sha256::digest(token.as_bytes());
    digest.iter().fold(String::with_capacity(64), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

/// In-memory one-time codes; expired entries are swept on every access
/// so the map can't grow via abandoned codes.
#[derive(Default)]
pub struct PairingStore {
    codes: Mutex<HashMap<String, u64>>,
}

impl PairingStore {
    /// Register an externally minted code (the `pair-code` file).
    pub fn register(&self, code: &str, expires_at: u64) {
        let normalized = normalize_pair_code(code);
        if normalized.is_empty() {
            return;
        }
        self.codes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(normalized, expires_at);
    }

    /// Mint + register a fresh code; returns what the operator sees.
    pub fn mint(&self, ttl_ms: u64) -> (String, u64) {
        let code = mint_pair_code();
        let expires_at = now_ms() + ttl_ms;
        self.register(&code, expires_at);
        (code, expires_at)
    }

    /// Single-use redeem: consumes the code on success; unknown, malformed
    /// or expired codes all return false — never distinguish the case.
    pub fn redeem(&self, code: &str) -> bool {
        let now = now_ms();
        let mut codes = self
            .codes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        codes.retain(|_, expires_at| *expires_at > now);
        codes.remove(&normalize_pair_code(code)).is_some()
    }
}

/// The on-disk mint channel: `{code, expiresAt}` JSON read on every
/// `POST /api/pair` and deleted on first read.
fn read_pair_code_file(path: &std::path::Path) -> Option<(String, u64)> {
    let raw = std::fs::read_to_string(path).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let record = parsed.as_object()?;
    let code = record.get("code")?.as_str()?;
    let expires_at = record.get("expiresAt")?.as_f64()?;
    if !expires_at.is_finite() {
        return None;
    }
    Some((code.to_string(), expires_at as u64))
}

/// The seam the app consumes: redeem a code, check a credential.
/// Filesystem-backed — `code_file` is the mint gate (whoever could write
/// it is the machine owner); `tokens_file` persists issued-credential
/// hashes across restarts.
#[derive(Clone)]
pub struct Pairing {
    inner: std::sync::Arc<PairingInner>,
}

struct PairingInner {
    code_file: PathBuf,
    tokens_file: PathBuf,
    store: PairingStore,
    hashes: Mutex<HashSet<String>>,
}

impl Pairing {
    /// Open the pairing backend; a corrupt tokens file degrades to empty.
    pub fn open(code_file: PathBuf, tokens_file: PathBuf) -> Self {
        let mut hashes = HashSet::new();
        if let Ok(raw) = std::fs::read_to_string(&tokens_file) {
            if let Ok(serde_json::Value::Object(record)) = serde_json::from_str(&raw) {
                if let Some(serde_json::Value::Array(tokens)) = record.get("tokens") {
                    for token in tokens {
                        if let Some(hash) = token.as_str() {
                            hashes.insert(hash.to_string());
                        }
                    }
                }
            }
        }
        Self {
            inner: std::sync::Arc::new(PairingInner {
                code_file,
                tokens_file,
                store: PairingStore::default(),
                hashes: Mutex::new(hashes),
            }),
        }
    }

    /// In-memory pairing for tests — no file channel.
    pub fn in_memory(store: PairingStore) -> Self {
        Self {
            inner: std::sync::Arc::new(PairingInner {
                code_file: PathBuf::new(),
                tokens_file: PathBuf::new(),
                store,
                hashes: Mutex::new(HashSet::new()),
            }),
        }
    }

    fn flush_tokens(&self) {
        let tmp = self
            .inner
            .tokens_file
            .with_extension(format!("tmp-{}", std::process::id()));
        let hashes = self
            .inner
            .hashes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let body = serde_json::json!({ "tokens": hashes.iter().collect::<Vec<_>>() });
        if let Some(dir) = self.inner.tokens_file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_vec(&body) {
            if std::fs::write(&tmp, &json).is_ok() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
                }
                let _ = std::fs::rename(&tmp, &self.inner.tokens_file);
            }
        }
    }

    /// Absorb a pending `pair-code` file — consume it regardless of
    /// contents so a stale code can't linger past its TTL.
    fn absorb_code_file(&self) {
        if !self.inner.code_file.exists() {
            return;
        }
        let file = read_pair_code_file(&self.inner.code_file);
        let _ = std::fs::remove_file(&self.inner.code_file);
        if let Some((code, expires_at)) = file {
            if expires_at > now_ms() {
                self.inner.store.register(&code, expires_at);
            }
        }
    }

    /// `POST /api/pair`: absorb any pending code file, then redeem. On
    /// success returns a fresh long-lived credential (already persisted).
    pub fn redeem(&self, code: &str) -> Option<String> {
        self.absorb_code_file();
        if !self.inner.store.redeem(code) {
            return None;
        }
        let token = mint_pair_token();
        {
            let mut hashes = self
                .inner
                .hashes
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            hashes.insert(hash_pair_token(&token));
        }
        if !self.inner.tokens_file.as_os_str().is_empty() {
            self.flush_tokens();
        }
        Some(token)
    }

    /// Bearer-auth check for credentials this node has issued.
    pub fn accepts(&self, token: &str) -> bool {
        self.inner
            .hashes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&hash_pair_token(token))
    }
}

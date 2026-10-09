//! The local-store verbs — `list`,
//! `export`, `import`, `install`, `delete` over the four on-disk agent
//! stores (devin/cline/claude/cursor), plus the `--*-dir` store-selector
//! resolution rules. No running node involved.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::ValueEnum;
use sepia_core::Session;
use sepia_core::storage::SessionRepository;
use sepia_core::wire::{session_from_json, session_to_json};

use crate::CliError;

/// `SessionRepository` futures are async even where the driver is sync —
/// the CLI is one-shot, a current-thread runtime per call is plenty.
pub fn block_on<F: std::future::Future>(rt: &tokio::runtime::Runtime, f: F) -> F::Output {
    rt.block_on(f)
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

fn default_db_path() -> PathBuf {
    home_dir().join(".local/share/devin/cli/sessions.db")
}

fn default_data_dir() -> PathBuf {
    home_dir().join(".cline/data")
}

fn default_claude_dir() -> PathBuf {
    home_dir().join(".claude")
}

fn default_cursor_dir() -> PathBuf {
    home_dir().join(".cursor")
}

/// The shared store flags — `--db` is always the Devin sessions DB; the
/// `--*-dir` flags double as store selectors (see [`dir_store`]).
#[derive(Clone, clap::Args)]
pub struct StoreFlags {
    /// Path to the Devin sessions SQLite database
    #[arg(long, default_value_os_t = default_db_path())]
    pub db: PathBuf,
    /// Path to the Cline CLI data directory
    #[arg(long)]
    pub data_dir: Option<PathBuf>,
    /// Path to the Claude Code data directory (its `projects/` tree is used)
    #[arg(long)]
    pub claude_dir: Option<PathBuf>,
    /// Path to the Cursor data directory
    #[arg(long)]
    pub cursor_dir: Option<PathBuf>,
}

impl From<StoreFlags> for StoreDirs {
    fn from(f: StoreFlags) -> Self {
        Self {
            db: f.db,
            data_dir: f.data_dir,
            claude_dir: f.claude_dir,
            cursor_dir: f.cursor_dir,
        }
    }
}

/// The four session stores the verbs read and write.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum StoreId {
    Devin,
    Cline,
    Claude,
    Cursor,
}

impl StoreId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Devin => "devin",
            Self::Cline => "cline",
            Self::Claude => "claude",
            Self::Cursor => "cursor",
        }
    }
}

/// The directories a command line resolved, ready for stores. The
/// `--*-dir` flags double as store selectors: passing one names that
/// store unless `--from`/`--to` says otherwise; when the store is
/// selected another way, a missing flag falls back to the agent's own
/// default dir.
#[derive(Clone, Debug)]
pub struct StoreDirs {
    pub db: PathBuf,
    pub data_dir: Option<PathBuf>,
    pub claude_dir: Option<PathBuf>,
    pub cursor_dir: Option<PathBuf>,
}

impl StoreDirs {
    fn data_dir(&self) -> PathBuf {
        self.data_dir.clone().unwrap_or_else(default_data_dir)
    }
    fn claude_dir(&self) -> PathBuf {
        self.claude_dir.clone().unwrap_or_else(default_claude_dir)
    }
    fn cursor_dir(&self) -> PathBuf {
        self.cursor_dir.clone().unwrap_or_else(default_cursor_dir)
    }
}

/// The store an explicit `--*-dir` names — the flag would be noise
/// otherwise. `claimed` excludes the store the sibling role (`--to` vs
/// `--from`) already resolved to, so `install --to claude --claude-dir X`
/// doesn't turn the source into claude too.
fn dir_store(dirs: &StoreDirs, claimed: Option<StoreId>) -> Option<StoreId> {
    if dirs.claude_dir.is_some() && claimed != Some(StoreId::Claude) {
        Some(StoreId::Claude)
    } else if dirs.cursor_dir.is_some() && claimed != Some(StoreId::Cursor) {
        Some(StoreId::Cursor)
    } else if dirs.data_dir.is_some() && claimed != Some(StoreId::Cline) {
        Some(StoreId::Cline)
    } else {
        None
    }
}

/// Resolve which store a role (`--from`/`--to`) names. An explicit flag
/// wins; otherwise an explicit `--*-dir` names its store; last resort is
/// the verb's own default.
fn resolve_store(
    flag: Option<StoreId>,
    dirs: &StoreDirs,
    fallback: StoreId,
    claimed: Option<StoreId>,
) -> StoreId {
    flag.or_else(|| dir_store(dirs, claimed))
        .unwrap_or(fallback)
}

/// The repository for `store`; devin opens read-only unless asked to
/// write.
fn repo(
    store: StoreId,
    dirs: &StoreDirs,
    write: bool,
) -> Result<Arc<dyn SessionRepository>, CliError> {
    match store {
        StoreId::Devin => {
            let store = sepia_driver_devin::DevinStore::open(&dirs.db, !write)
                .map_err(|e| CliError(format!("Failed to open database: {}", e.message)))?;
            Ok(Arc::new(store))
        }
        StoreId::Cline => Ok(Arc::new(sepia_driver_cline::ClineStore::new(
            dirs.data_dir(),
        ))),
        StoreId::Claude => Ok(Arc::new(sepia_driver_claude::ClaudeStore::new(
            dirs.claude_dir().join("projects"),
        ))),
        StoreId::Cursor => Ok(Arc::new(sepia_driver_cursor::CursorStore::new(
            dirs.cursor_dir(),
        ))),
    }
}

fn storage_err(e: sepia_core::StorageError) -> CliError {
    CliError(e.message)
}

/// `session` re-keyed for `--id`.
fn rename_session(session: &Session, id: Option<&str>) -> Session {
    match id {
        Some(id) if id != session.id => Session {
            id: id.to_string(),
            ..session.clone()
        },
        _ => session.clone(),
    }
}

/// Write `session` into `store`. Devin goes through `import_session`
/// (cogs grafting, existing-session skip); cline through
/// `ClineStore::install` (live-owner guard, index row, generated id when
/// `--id` is absent); claude/cursor through `repo.save` — an existing id
/// refuses unless `--force`, since a save rewrites the store in place.
fn install_into(
    rt: &tokio::runtime::Runtime,
    store: StoreId,
    dirs: &StoreDirs,
    session: &Session,
    id: Option<&str>,
    force: bool,
    imported_log: &str,
) -> Result<String, CliError> {
    // `--id` renames for the stores that key the write on session.id.
    let renamed = rename_session(session, id);
    match store {
        StoreId::Devin => {
            let repo = repo(StoreId::Devin, dirs, true)?;
            let exists = block_on(rt, repo.has_session(&renamed.id)).map_err(storage_err)?;
            if exists {
                println!(
                    "Session {} is already imported; leaving it untouched",
                    renamed.id
                );
                return Ok(renamed.id.clone());
            }
            block_on(rt, sepia_convert::import_session(&repo, &renamed))
                .map_err(|e| CliError(format!("Import failed: {}", e.message)))?;
            println!("{imported_log}");
            Ok(renamed.id)
        }
        StoreId::Cline => {
            let installed = id.map_or_else(
                || sepia_driver_cline::cline::cline_session_id(session.created_at * 1000.0),
                str::to_string,
            );
            let store = sepia_driver_cline::ClineStore::new(dirs.data_dir());
            store
                .install(session, &installed, force)
                .map_err(|e| CliError(format!("Install failed: {}", e.message)))?;
            println!(
                "Installed session {} into {}",
                installed,
                dirs.data_dir().display()
            );
            println!("Resume it with: cline --id {installed} -m <model>");
            Ok(installed)
        }
        StoreId::Claude | StoreId::Cursor => {
            let repo = repo(store, dirs, false)?;
            let exists = block_on(rt, repo.has_session(&renamed.id)).map_err(storage_err)?;
            if exists && !force {
                return Err(CliError(format!(
                    "Session {} already exists in the {} store; pass --force to overwrite",
                    renamed.id,
                    store.as_str()
                )));
            }
            block_on(rt, repo.save(&renamed)).map_err(storage_err)?;
            println!(
                "Installed session {} into the {} store",
                renamed.id,
                store.as_str()
            );
            Ok(renamed.id)
        }
    }
}

/// Fetch a session from `store`, failing with the shared not-found error.
fn get_session(
    rt: &tokio::runtime::Runtime,
    store: StoreId,
    dirs: &StoreDirs,
    session_id: &str,
) -> Result<Session, CliError> {
    let repo = repo(store, dirs, false)?;
    block_on(rt, repo.get_by_id(session_id, None))
        .map_err(storage_err)?
        .ok_or_else(|| CliError(format!("Session not found: {session_id}")))
}

/// How `read_session_path` decoded the source — the log lines name it.
#[derive(Clone, Copy)]
enum ImportedKind {
    Cline,
    Claude,
    Json,
}

impl ImportedKind {
    fn label(self) -> &'static str {
        match self {
            Self::Cline => "Cline",
            Self::Claude => "Claude",
            Self::Json => "JSON",
        }
    }
}

/// Read a session off disk for `import` — a Cline session directory, a
/// Claude `.jsonl` transcript, or a `SessionJson` export file (what
/// `sepia export` and `GET /api/sessions/:id/export` produce).
fn read_session_path(
    input_path: &Path,
    session_id: Option<&str>,
) -> Result<(Session, ImportedKind), CliError> {
    let info = std::fs::metadata(input_path)
        .map_err(|_| CliError(format!("Import source not found: {}", input_path.display())))?;
    if info.is_dir() {
        let session = sepia_driver_cline::cline::from_directory(input_path, session_id)
            .map_err(|e| CliError(e.message))?;
        return Ok((session, ImportedKind::Cline));
    }
    if input_path.to_string_lossy().ends_with(".jsonl") {
        let session = sepia_driver_claude::from_file(input_path, session_id, None)
            .map_err(|e| CliError(e.message))?;
        return Ok((session, ImportedKind::Claude));
    }
    let raw = std::fs::read_to_string(input_path)
        .map_err(|_| CliError(format!("Failed to read {}", input_path.display())))?;
    let parsed: serde_json::Value = serde_json::from_str(&raw).map_err(|_| {
        CliError(format!(
            "Import source is neither a Cline dir, a Claude .jsonl, nor a session JSON: {}",
            input_path.display()
        ))
    })?;
    let session = session_from_json(&parsed).map_err(|_| {
        CliError(format!(
            "Import source is neither a Cline dir, a Claude .jsonl, nor a session JSON: {}",
            input_path.display()
        ))
    })?;
    Ok((rename_session(&session, session_id), ImportedKind::Json))
}

/// `sepia import <path>` — decode the source, write it into `--to`
/// (default devin).
pub fn import_session_cmd(
    rt: &tokio::runtime::Runtime,
    source: &Path,
    dirs: &StoreDirs,
    to: Option<StoreId>,
    session_id: Option<&str>,
    force: bool,
) -> Result<(), CliError> {
    let target = resolve_store(to, dirs, StoreId::Devin, None);
    let (session, kind) = read_session_path(source, session_id)?;
    install_into(
        rt,
        target,
        dirs,
        &session,
        session_id,
        force,
        &format!(
            "Imported {} session {} into storage",
            kind.label(),
            session.id
        ),
    )?;
    Ok(())
}

/// `sepia export <session-id> [out]` — the session IR JSON (default) or
/// Cline session files (`--format cline` requires `out` as a directory).
pub fn export_session_cmd(
    rt: &tokio::runtime::Runtime,
    session_id: &str,
    out: Option<&Path>,
    format: ExportFormat,
    dirs: &StoreDirs,
    from: Option<StoreId>,
) -> Result<(), CliError> {
    let source = resolve_store(from, dirs, StoreId::Devin, None);
    if format == ExportFormat::Cline {
        let Some(out_path) = out else {
            return Err(CliError(
                "export --format cline needs an out directory".into(),
            ));
        };
        let repo = repo(source, dirs, false)?;
        block_on(
            rt,
            sepia_convert::export_cline(&repo, session_id, out_path, false, false),
        )
        .map_err(|e| CliError(e.message))?;
        println!("Exported session {session_id} to {}", out_path.display());
        return Ok(());
    }
    let session = get_session(rt, source, dirs, session_id)?;
    let json = serde_json::to_string_pretty(
        &session_to_json(&session)
            .map_err(|e| CliError(format!("Failed to encode session {session_id}: {e}")))?,
    )
    .map_err(|e| CliError(format!("Failed to encode session {session_id}: {e}")))?;
    match out {
        None => println!("{json}"),
        Some(o) if o.as_os_str() == "-" => println!("{json}"),
        Some(o) => {
            let file_path = if o.is_dir() {
                o.join(format!("{}.session.json", session.id))
            } else {
                o.to_path_buf()
            };
            std::fs::write(&file_path, format!("{json}\n"))
                .map_err(|e| CliError(format!("Failed to write {}: {e}", file_path.display())))?;
            println!("Exported session {session_id} to {}", file_path.display());
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ExportFormat {
    Json,
    Cline,
}

/// `sepia install <session-id>` — copy a session into a store ready to
/// resume: cline (default), claude, cursor or devin.
#[allow(clippy::too_many_arguments)]
pub fn install_session_cmd(
    rt: &tokio::runtime::Runtime,
    session_id: &str,
    dirs: &StoreDirs,
    from: Option<StoreId>,
    to: Option<StoreId>,
    id: Option<&str>,
    force: bool,
) -> Result<(), CliError> {
    let target = resolve_store(to, dirs, StoreId::Cline, None);
    let source = resolve_store(from, dirs, StoreId::Devin, Some(target));
    let session = get_session(rt, source, dirs, session_id)?;
    install_into(
        rt,
        target,
        dirs,
        &session,
        id,
        force,
        &format!(
            "Installed session {} into the devin store",
            id.unwrap_or(&session.id)
        ),
    )?;
    Ok(())
}

/// `sepia delete <session-id>` — remove it from the resolved store.
pub fn delete_session_cmd(
    rt: &tokio::runtime::Runtime,
    session_id: &str,
    dirs: &StoreDirs,
    from: Option<StoreId>,
) -> Result<(), CliError> {
    let source = resolve_store(from, dirs, StoreId::Devin, None);
    let repo = repo(source, dirs, true)?;
    block_on(rt, repo.delete(session_id)).map_err(storage_err)?;
    println!(
        "Deleted session {session_id} from the {} store",
        source.as_str()
    );
    Ok(())
}

/// `sepia list` — `id<TAB>title<TAB>workingDirectory` rows, or the
/// `No sessions found` line.
pub fn list_sessions_cmd(
    rt: &tokio::runtime::Runtime,
    dirs: &StoreDirs,
    from: Option<StoreId>,
) -> Result<(), CliError> {
    let source = resolve_store(from, dirs, StoreId::Devin, None);
    let repo = repo(source, dirs, false)?;
    let sessions = block_on(rt, repo.list()).map_err(storage_err)?;
    if sessions.is_empty() {
        println!("No sessions found");
    } else {
        for s in &sessions {
            println!("{}\t{}\t{}", s.id, s.title, s.working_directory);
        }
    }
    Ok(())
}

//! sepia — the CLI for the agent-session abstraction. Local store verbs
//! over the Devin/Cline/Claude/Cursor stores, agent config IR verbs,
//! driver discovery, and `serve` — the ndjson JSON-RPC session-plane
//! service orchestrators (peregrin) spawn and drive over stdio.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

use sepia_cli::store::{ExportFormat, StoreFlags, StoreId};
use sepia_cli::{CliError, SEPIA_VERSION, config_ops, serve, store};

// --- config --------------------------------------------------------------------

/// The `--*-dir` flags selecting/overriding local agent config dirs.
/// Field names must keep the `dir` postfix — it is the flag name.
#[allow(clippy::struct_field_names)]
#[derive(Clone, Args)]
struct ConfigDirFlags {
    /// Path to a .claude dir (default: ~/.claude)
    #[arg(long)]
    claude_dir: Option<PathBuf>,
    /// Path to a .cursor dir (default: ~/.cursor)
    #[arg(long)]
    cursor_dir: Option<PathBuf>,
    /// Path to a Cline workspace root (default: cwd)
    #[arg(long)]
    cline_dir: Option<PathBuf>,
    /// Path to a Devin config dir (default: ~/.config/devin)
    #[arg(long)]
    devin_dir: Option<PathBuf>,
}

impl From<&ConfigDirFlags> for config_ops::ConfigDirs {
    fn from(f: &ConfigDirFlags) -> Self {
        Self {
            claude: f.claude_dir.clone(),
            cursor: f.cursor_dir.clone(),
            cline: f.cline_dir.clone(),
            devin: f.devin_dir.clone(),
        }
    }
}

use config_ops::AgentId;

#[derive(Subcommand)]
enum ConfigCommands {
    /// List an agent's config items (rules, skills, commands, hooks,
    /// subagents, MCP)
    List {
        /// Agent to read the config from
        #[arg(long)]
        from: Option<AgentId>,
        #[command(flatten)]
        dirs: ConfigDirFlags,
    },
    /// Export an agent's config as IR JSON (stdout or a file)
    Export {
        /// Output file for the config JSON; stdout when omitted
        out: Option<PathBuf>,
        /// Agent to read the config from
        #[arg(long)]
        from: Option<AgentId>,
        #[command(flatten)]
        dirs: ConfigDirFlags,
    },
    /// Install a config IR JSON file into an agent's config store
    Import {
        /// Config JSON file produced by `sepia config export`
        path: PathBuf,
        /// Agent to write the config into
        #[arg(long)]
        to: Option<AgentId>,
        #[command(flatten)]
        dirs: ConfigDirFlags,
    },
    /// Copy one agent's config into another's store (IR → target's files)
    Install {
        /// Agent to read the config from
        #[arg(long)]
        from: Option<AgentId>,
        /// Agent to write the config into
        #[arg(long)]
        to: Option<AgentId>,
        #[command(flatten)]
        dirs: ConfigDirFlags,
    },
    /// Compare two agents' configs (name-level, plus hook/MCP deltas)
    Diff {
        /// Agent to read the config from
        #[arg(long)]
        from: Option<AgentId>,
        /// Agent to write the config into
        #[arg(long)]
        to: Option<AgentId>,
        #[command(flatten)]
        dirs: ConfigDirFlags,
    },
}

// --- drivers -----------------------------------------------------------------

#[derive(Subcommand)]
enum DriverCommands {
    /// List the sepia-driver-* binaries discovery finds, their probed
    /// manifests, and whether each agent command resolves
    List {
        /// Print the probed manifests (plus probe problems) as JSON
        #[arg(long)]
        json: bool,
    },
}

// --- store -------------------------------------------------------------------

/// The local-store verbs — read and write agent session stores directly
/// (no daemon needed).
#[derive(Subcommand)]
enum StoreCommands {
    /// List sessions in a store
    List(StoreListArgs),
    /// Export a session from a store — the session IR JSON (default) or
    /// Cline session files
    Export(StoreExportArgs),
    /// Import a session (Cline dir, Claude .jsonl, or session JSON) into
    /// a store
    Import(StoreImportArgs),
    /// Copy a session into a store ready to resume — cline (default),
    /// claude, cursor or devin
    Install(StoreInstallArgs),
    /// Delete a session from a store
    Delete(StoreDeleteArgs),
}

#[derive(Clone, Args)]
struct StoreListArgs {
    #[command(flatten)]
    dirs: StoreFlags,
    /// Store to read the session from (devin, cline, claude, cursor)
    #[arg(long)]
    from: Option<StoreId>,
}

#[derive(Clone, Args)]
struct StoreExportArgs {
    /// Session id on the store
    session_id: String,
    /// Output file (or directory) for the session JSON; stdout when
    /// omitted — required as a directory for --format cline
    out: Option<PathBuf>,
    /// Export format: the session IR JSON, or Cline session files
    #[arg(long, default_value = "json")]
    format: ExportFormat,
    #[command(flatten)]
    dirs: StoreFlags,
    /// Store to read the session from (devin, cline, claude, cursor)
    #[arg(long)]
    from: Option<StoreId>,
}

#[derive(Clone, Args)]
struct StoreImportArgs {
    /// Session to import — a Cline session dir, a Claude .jsonl
    /// transcript, or a session JSON export
    source: PathBuf,
    /// Store to write the session into (devin, cline, claude, cursor)
    #[arg(long)]
    to: Option<StoreId>,
    /// Override the imported session id
    #[arg(long)]
    session_id: Option<String>,
    /// Overwrite a session that already exists in the target store
    #[arg(long)]
    force: bool,
    #[command(flatten)]
    dirs: StoreFlags,
}

#[derive(Clone, Args)]
struct StoreInstallArgs {
    /// Session id on the source store
    session_id: String,
    /// Store to read the session from (devin, cline, claude, cursor)
    #[arg(long)]
    from: Option<StoreId>,
    /// Store to write the session into (devin, cline, claude, cursor)
    #[arg(long)]
    to: Option<StoreId>,
    /// Session id to install under (defaults to a generated one)
    #[arg(long)]
    id: Option<String>,
    /// Replace a session that still belongs to a live owner
    #[arg(long)]
    force: bool,
    #[command(flatten)]
    dirs: StoreFlags,
}

#[derive(Clone, Args)]
struct StoreDeleteArgs {
    /// Session id on the store
    session_id: String,
    /// Store to read the session from (devin, cline, claude, cursor)
    #[arg(long)]
    from: Option<StoreId>,
    #[command(flatten)]
    dirs: StoreFlags,
}

// --- root ----------------------------------------------------------------------

/// Inspect and manage coding-agent sessions agnostically — local stores,
/// agent configs, drivers, and the session plane itself.
#[derive(Parser)]
#[command(name = "sepia", version = SEPIA_VERSION)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Local driver discovery — the sepia-driver-* binaries the session
    /// plane loads (SEPIA_DRIVER_DIR → platform data dir → PATH).
    /// `sepia driver` alone lists them.
    Driver {
        #[command(subcommand)]
        command: Option<DriverCommands>,
    },
    /// Agent config IR ops — list/export/import/install/diff rules,
    /// skills, commands, hooks, subagents and MCP servers
    #[command(subcommand)]
    Config(ConfigCommands),
    /// Store ops — read and write local agent stores directly
    #[command(subcommand)]
    Store(StoreCommands),
    /// List sessions in a store
    List(StoreListArgs),
    /// Export a session from a store — the session IR JSON (default) or
    /// Cline session files
    Export(StoreExportArgs),
    /// Import a session (Cline dir, Claude .jsonl, or session JSON) into
    /// a store
    Import(StoreImportArgs),
    /// Copy a session into a store ready to resume — cline (default),
    /// claude, cursor or devin
    Install(StoreInstallArgs),
    /// Delete a session from a store
    Delete(StoreDeleteArgs),
    /// Serve the local session plane — ndjson JSON-RPC over stdio
    /// (spawned and driven by orchestrators like peregrin)
    Serve,
    /// Print the sepia version stamp
    Version,
}

/// One runtime for the whole command — store drivers are sync under the
/// async trait; `serve` builds its own multi-thread runtime.
fn runtime() -> Result<tokio::runtime::Runtime, CliError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| CliError(format!("failed to start runtime: {e}")))
}

fn run_store(cmd: &StoreCommands, rt: &tokio::runtime::Runtime) -> Result<(), CliError> {
    match cmd {
        StoreCommands::List(a) => store::list_sessions_cmd(rt, &a.dirs.clone().into(), a.from),
        StoreCommands::Export(a) => store::export_session_cmd(
            rt,
            &a.session_id,
            a.out.as_deref(),
            a.format,
            &a.dirs.clone().into(),
            a.from,
        ),
        StoreCommands::Import(a) => store::import_session_cmd(
            rt,
            &a.source,
            &a.dirs.clone().into(),
            a.to,
            a.session_id.as_deref(),
            a.force,
        ),
        StoreCommands::Install(a) => store::install_session_cmd(
            rt,
            &a.session_id,
            &a.dirs.clone().into(),
            a.from,
            a.to,
            a.id.as_deref(),
            a.force,
        ),
        StoreCommands::Delete(a) => {
            store::delete_session_cmd(rt, &a.session_id, &a.dirs.clone().into(), a.from)
        }
    }
}

fn run_config(cmd: &ConfigCommands) -> Result<(), CliError> {
    match cmd {
        ConfigCommands::List { from, dirs } => config_ops::config_list(*from, &dirs.into()),
        ConfigCommands::Export { out, from, dirs } => {
            config_ops::config_export(out.as_deref(), *from, &dirs.into())
        }
        ConfigCommands::Import { path, to, dirs } => {
            config_ops::config_import(path, *to, &dirs.into())
        }
        ConfigCommands::Install { from, to, dirs } => {
            config_ops::config_install(*from, *to, &dirs.into())
        }
        ConfigCommands::Diff { from, to, dirs } => {
            config_ops::config_diff(*from, *to, &dirs.into())
        }
    }
}

fn dispatch(cli: &Cli, rt: &tokio::runtime::Runtime) -> Result<(), CliError> {
    match &cli.command {
        Commands::Driver { command } => {
            let json = matches!(command, Some(DriverCommands::List { json: true }));
            sepia_cli::driver::list(rt, json)
        }
        Commands::Config(cmd) => run_config(cmd),
        Commands::Store(cmd) => run_store(cmd, rt),
        Commands::List(a) => run_store(&StoreCommands::List(a.clone()), rt),
        Commands::Export(a) => run_store(&StoreCommands::Export(a.clone()), rt),
        Commands::Import(a) => run_store(&StoreCommands::Import(a.clone()), rt),
        Commands::Install(a) => run_store(&StoreCommands::Install(a.clone()), rt),
        Commands::Delete(a) => run_store(&StoreCommands::Delete(a.clone()), rt),
        Commands::Serve => {
            let _telemetry = sepia_telemetry::init();
            serve::serve_stdio()
        }
        Commands::Version => {
            println!("{SEPIA_VERSION}");
            Ok(())
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match runtime().and_then(|rt| dispatch(&cli, &rt)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

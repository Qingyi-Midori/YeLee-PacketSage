//! Command line surface (M0~M2 §5.1, 开发文档 §22, CLI 收口工程规格书 §3/§5/§6/§8).
//!
//! Two rules shape this file:
//!
//! * every flag carries a doc comment — `--help` is the user manual, and the
//!   help text is covered by a snapshot test (§3.1);
//! * the prog name is pinned to `packetsage` so help output cannot drift with
//!   the file name or the invocation path (§2.2, C7).

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// YeLee' PacketSage — deterministic capture analysis with an evidence chain.
#[derive(Debug, Parser)]
#[command(
    name = "packetsage",
    bin_name = "packetsage",
    before_help = crate::banner::TITLE,
    about = "PCAP/PCAPNG analysis engine (Rust) with a JSONL RPC worker",
    disable_help_subcommand = true,
    disable_version_flag = true,
    arg_required_else_help = true,
    after_help = "Examples:\n  \
        packetsage analyze samples/synth-mixed.pcap\n  \
        packetsage analyze samples/synth-mixed.pcap --jsonl > events.jsonl\n  \
        packetsage db query --readonly --sql \"SELECT id, status FROM analysis_tasks\"\n\n\
        Exit codes: 0 ok, 1 usage, 2 capture, 3 config/db/llm, 4 internal, 5 unsupported."
)]
pub struct Cli {
    /// Print the version line and exit (`-V`).
    #[arg(long, short = 'V', global = true)]
    pub version: bool,
    /// Increase log verbosity on stderr: `-v` = info, `-vv` = debug + config dump.
    #[arg(long, short = 'v', global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,
    /// Quiet: no progress and no info logs (errors and warnings still reach stderr).
    #[arg(long, short = 'q', global = true)]
    pub quiet: bool,
    /// Explicit configuration file (overrides $PACKETSAGE_CONFIG and ./packetsage.yaml).
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,
    /// Disable ANSI styling (also honoured via a non-empty $NO_COLOR).
    #[arg(long = "no-color", global = true)]
    pub no_color: bool,
    /// Sub-command.
    #[command(subcommand)]
    pub command: Option<Command>,
}

/// Top level sub-commands.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Print the engine version, event schema version and build metadata.
    Version(VersionArgs),
    /// Check the local installation (10 checks, `--json` for scripts).
    Doctor(DoctorArgs),
    /// Analyse a capture file.
    Analyze(AnalyzeArgs),
    /// Run the JSONL RPC worker on stdin/stdout.
    Serve,
    /// Query an analysed task from the database.
    Query(QueryArgs),
    /// Inspect or migrate the database (read-only queries only).
    Db(DbArgs),
    /// List or validate rules.
    Rules(RulesArgs),
    /// Print the event / RPC schema digest.
    Schema,
}

/// `version` arguments.
#[derive(Debug, Args)]
pub struct VersionArgs {
    /// Emit the stable machine readable form (json output format v1).
    #[arg(long)]
    pub json: bool,
}

/// `doctor` arguments.
#[derive(Debug, Args)]
#[command(after_help = "Example:\n  \
    packetsage doctor\n  \
    packetsage doctor --json --no-net")]
pub struct DoctorArgs {
    /// Rules directory to load (CLI > $PACKETSAGE_RULES_PATH > config > `rules`).
    #[arg(long, value_name = "DIR")]
    pub rules_dir: Option<PathBuf>,
    /// Samples directory to probe.
    #[arg(long, value_name = "DIR")]
    pub samples_dir: Option<PathBuf>,
    /// Database URL to verify.
    #[arg(long, value_name = "URL")]
    pub db: Option<String>,
    /// Skip the checks that need the network (only the reachability probe of item 8).
    #[arg(long)]
    pub no_net: bool,
    /// Emit the report as JSON (stable json output format v1).
    #[arg(long)]
    pub json: bool,
}

/// `analyze` arguments.
#[derive(Debug, Args)]
#[command(after_help = "Example:\n  \
    packetsage analyze samples/synth-mixed.pcap\n  \
    packetsage analyze capture.pcap --jsonl > events.jsonl")]
pub struct AnalyzeArgs {
    /// Capture file (pcap / pcapng).
    pub capture: PathBuf,
    /// Pin the task id (`task_<ulid>`) instead of generating one.
    ///
    /// Useful for reproducible pipelines, for re-running an analysis under the
    /// same identity, and for the event-stream determinism test.
    #[arg(long, value_name = "ID")]
    pub task_id: Option<String>,
    /// Emit the JSONL event stream on stdout (this sub-command's machine stream).
    #[arg(long)]
    pub jsonl: bool,
    /// Write the event stream to a file instead of stdout.
    #[arg(long, value_name = "PATH")]
    pub json: Option<PathBuf>,
    /// Limit the number of emitted events (0 = unlimited).
    #[arg(long, default_value_t = 0)]
    pub limit_events: u64,
    /// Rules directory.
    #[arg(long, value_name = "DIR")]
    pub rules_dir: Option<PathBuf>,
    /// Disable the rule engine.
    #[arg(long)]
    pub no_rules: bool,
    /// Run the full pipeline (rules + agent + report).
    #[arg(long)]
    pub full: bool,
    /// Persist the task into a database.
    #[arg(long, value_name = "URL")]
    pub db: Option<String>,
    /// Report path for `--full`.
    #[arg(long, value_name = "PATH")]
    pub report: Option<PathBuf>,
    /// Python interpreter used by `--full`.
    #[arg(long, default_value = "python")]
    pub python: String,
    /// Provider used by the agent stage (`mock`, `openai`, `local`).
    #[arg(long)]
    pub provider: Option<String>,
    /// Model name for the agent stage.
    #[arg(long)]
    pub model: Option<String>,
    /// Emit only packets that carry decode errors.
    #[arg(long)]
    pub errors_only: bool,
}

/// `query` arguments.
#[derive(Debug, Args)]
pub struct QueryArgs {
    /// What to query.
    #[command(subcommand)]
    pub kind: QueryKind,
    /// Database URL.
    #[arg(long, global = true, value_name = "URL")]
    pub db: Option<String>,
    /// Restrict to one task.
    #[arg(long, global = true)]
    pub task_id: Option<String>,
    /// Emit the rows as JSON lines on stdout (this sub-command's machine stream).
    #[arg(long, global = true)]
    pub jsonl: bool,
    /// Write the JSON lines to a file instead of printing the human table.
    #[arg(long, global = true, value_name = "PATH")]
    pub json: Option<PathBuf>,
}

/// Query kinds.
#[derive(Debug, Subcommand)]
pub enum QueryKind {
    /// List sessions.
    Sessions {
        /// Sort key: bytes | packets | duration.
        #[arg(long, default_value = "bytes")]
        sort_by: String,
        /// Maximum rows.
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// List protocol statistics.
    Stats {
        /// Layer: link | ipv4 | ipv6 | tcp | udp | icmp | dns | http | tls | dhcp.
        #[arg(long, default_value = "tcp")]
        layer: String,
    },
    /// List alerts.
    Alerts {
        /// Severity filter.
        #[arg(long)]
        severity: Option<String>,
        /// Rule id filter.
        #[arg(long)]
        rule_id: Option<String>,
        /// Maximum rows.
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// List stored findings.
    Findings {
        /// Maximum rows.
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
}

/// `db` arguments (ADR-018: the CLI never becomes a second writer).
#[derive(Debug, Args)]
#[command(after_help = "Example:\n  \
    packetsage db query --readonly --db \"sqlite://packetsage.db?mode=ro\" --sql \"SELECT id FROM analysis_tasks\"\n  \
    packetsage db migrate --yes")]
pub struct DbArgs {
    /// What to do.
    #[command(subcommand)]
    pub kind: DbKind,
    /// Database URL.
    #[arg(long, global = true, value_name = "URL")]
    pub db: Option<String>,
}

/// Database sub-commands.
#[derive(Debug, Subcommand)]
pub enum DbKind {
    /// Run a read-only SQL statement (`--readonly` is mandatory, fail-closed).
    Query(DbQueryArgs),
    /// Apply the pending migrations (interactive confirmation, or `--yes`).
    Migrate(DbMigrateArgs),
}

/// `db query` arguments.
#[derive(Debug, Args)]
#[command(after_help = "The URL must contain `mode=ro`; this command never creates a database.")]
pub struct DbQueryArgs {
    /// Acknowledge the read-only contract; without it the query is refused.
    #[arg(long)]
    pub readonly: bool,
    /// The statement to run (`SELECT` / `WITH` / `EXPLAIN` only).
    #[arg(long, value_name = "SQL")]
    pub sql: String,
    /// Maximum rows (default 200, hard upper bound 10000).
    #[arg(long, default_value_t = 200)]
    pub limit: u64,
    /// Connection and lock-wait timeout in seconds.
    #[arg(long, default_value_t = 10)]
    pub timeout: u64,
    /// Emit one JSON object per row (this sub-command's machine stream).
    #[arg(long)]
    pub jsonl: bool,
}

/// `db migrate` arguments.
#[derive(Debug, Args)]
pub struct DbMigrateArgs {
    /// Skip the confirmation prompt (required when stdin is not a terminal).
    #[arg(long)]
    pub yes: bool,
}

/// `rules` arguments.
#[derive(Debug, Args)]
pub struct RulesArgs {
    /// What to do.
    #[command(subcommand)]
    pub kind: RulesKind,
}

/// Rules sub-commands.
#[derive(Debug, Subcommand)]
pub enum RulesKind {
    /// List the rules that would be loaded.
    List {
        /// Directory to scan.
        #[arg(long, value_name = "DIR")]
        dir: Option<PathBuf>,
    },
    /// Validate rules in strict mode.
    Check {
        /// File or directory.
        path: PathBuf,
    },
}


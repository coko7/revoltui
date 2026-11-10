use clap::Parser;
use std::path::PathBuf;

/// Show current and upcoming mobilisations from manifsendirect.fr.
///
/// The feed is cached locally and re-downloaded when older than MIN minutes,
/// both at startup and while the TUI runs.
///
/// Env: MANIFS_URL overrides the source URL.
#[derive(Parser)]
#[command(version)]
pub struct Cli {
    /// City filter: case- and accent-insensitive, partial match ("lyon", "st-nazaire", "baule")
    pub city: Vec<String>,

    /// Local copy of events.json [default: ~/.cache/manifsendirect/events.json]. "-" reads stdin
    #[arg(short, long, value_name = "FILE")]
    pub file: Option<PathBuf>,

    /// Max age in minutes before re-downloading
    #[arg(
        short = 'm',
        long = "max-age",
        value_name = "MIN",
        default_value_t = 30
    )]
    pub max_age: u64,

    /// Force a re-download now
    #[arg(short, long, conflicts_with = "offline")]
    pub refresh: bool,

    /// Never download, use the local copy as is
    #[arg(short = 'n', long)]
    pub offline: bool,

    /// Also show finished events
    #[arg(short, long)]
    pub all: bool,
}

//! revoltui — browse current and upcoming mobilisations from manifsendirect.fr.

mod app;
mod data;
mod ui;

use std::io::{self, IsTerminal, Read};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::Parser;

use app::{App, Options};
use data::Origin;

/// Show current and upcoming mobilisations from manifsendirect.fr.
///
/// The feed is cached locally and re-downloaded when older than MIN minutes,
/// both at startup and while the TUI runs.
///
/// Env: MANIFS_URL overrides the source URL.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// City filter: case- and accent-insensitive, partial match ("lyon", "st-nazaire", "baule")
    city: Vec<String>,

    /// Local copy of events.json [default: ~/.cache/manifsendirect/events.json]. "-" reads stdin
    #[arg(short, long, value_name = "FILE")]
    file: Option<PathBuf>,

    /// Max age in minutes before re-downloading
    #[arg(short = 'm', long = "max-age", value_name = "MIN", default_value_t = 30)]
    max_age: u64,

    /// Force a re-download now
    #[arg(short, long, conflicts_with = "offline")]
    refresh: bool,

    /// Never download, use the local copy as is
    #[arg(short = 'n', long)]
    offline: bool,

    /// Also show finished events
    #[arg(short, long)]
    all: bool,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    let url = std::env::var("MANIFS_URL")
        .ok()
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| data::DEFAULT_URL.to_string());
    let max_age = Duration::from_secs(cli.max_age.saturating_mul(60));
    let origin = match cli.file {
        Some(p) if p.as_os_str() == "-" => Origin::Stdin,
        Some(p) => Origin::File(p),
        None => Origin::File(data::default_cache_file()),
    };

    let (feed, warning) = load(&origin, &url, max_age, cli.refresh, cli.offline)?;

    if !io::stdout().is_terminal() {
        return Err("stdout is not a terminal (use manif.sh for plain-text output)".into());
    }

    let opts = Options { origin, url, max_age, offline: cli.offline };
    let mut app = App::new(feed, opts, cli.city.join(" "), cli.all, warning);
    ratatui::run(|terminal| app.run(terminal)).map_err(|e| e.to_string())
}

/// Initial load, before the TUI starts: download when missing, forced or stale,
/// falling back to the older local copy with a warning.
fn load(
    origin: &Origin,
    url: &str,
    max_age: Duration,
    force: bool,
    offline: bool,
) -> Result<(data::Feed, Option<String>), String> {
    let path = match origin {
        Origin::Stdin => {
            let mut buf = Vec::new();
            io::stdin()
                .read_to_end(&mut buf)
                .map_err(|e| format!("cannot read stdin: {e}"))?;
            return Ok((data::parse(&buf)?, None));
        }
        Origin::File(path) => path,
    };

    if !offline && (force || data::is_stale(path, max_age)) {
        match data::download(url, path) {
            Ok(feed) => return Ok((feed, None)),
            Err(e) if data::has_local_copy(path) => {
                let warning = format!("téléchargement échoué, ancienne copie locale utilisée — {e}");
                return Ok((data::read_file(path)?, Some(warning)));
            }
            Err(e) => {
                return Err(format!(
                    "could not download and no local copy at {}: {e}",
                    path.display()
                ));
            }
        }
    }
    Ok((data::read_file(path)?, None))
}

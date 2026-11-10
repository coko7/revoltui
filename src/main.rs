mod app;
mod cli;
mod data;
mod ui;

use std::io::{self, IsTerminal, Read};
use std::process::ExitCode;
use std::time::Duration;

use app::{App, Options};
use clap::Parser;
use data::Origin;

use crate::cli::Cli;

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
        return Err("stdout is not a terminal".into());
    }

    let opts = Options {
        origin,
        url,
        max_age,
        offline: cli.offline,
    };
    let mut app = App::new(feed, opts, cli.city.join(" "), cli.all, warning);
    app.set_show_cities(!cli.hide_cities);
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
                let warning =
                    format!("téléchargement échoué, ancienne copie locale utilisée — {e}");
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

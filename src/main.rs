mod app;
mod cli;
mod config;
mod launcher;

use std::process::ExitCode;

use clap::Parser;

use crate::cli::Cli;
use crate::config::ConfigStore;

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Err(message) = cli.validate_forwarded_args() {
        eprintln!("codex-mcp: {message}");
        return ExitCode::from(2);
    }
    let codex_home = match cli.resolve_codex_home() {
        Ok(path) => path,
        Err(message) => {
            eprintln!("codex-mcp: {message}");
            return ExitCode::from(2);
        }
    };
    let store = ConfigStore::new(codex_home.clone());
    let mut launch_error = None;

    loop {
        match app::run(&store, &cli.codex_bin, launch_error.take()) {
            Ok(app::Outcome::Quit) => return ExitCode::SUCCESS,
            Ok(app::Outcome::Launch(target)) => {
                let error = launcher::launch(&cli.codex_bin, &codex_home, &target, &cli.codex_args);
                launch_error = Some(format!("could not launch Codex: {error}"));
            }
            Err(error) => {
                eprintln!("codex-mcp: {error}");
                return ExitCode::FAILURE;
            }
        }
    }
}

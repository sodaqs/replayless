mod cli;
mod config;
mod scan;

use anyhow::Result;
use clap::Parser;

use crate::cli::{Cli, Command};
use crate::config::Config;

fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    let cfg = Config::load(cli.config.as_deref())?;

    match cli.command {
        Command::Scan => scan::run(&cfg)?,
    }
    Ok(())
}

fn init_tracing(verbose: bool) {
    use tracing_subscriber::{EnvFilter, fmt};

    let default = if verbose { "debug" } else { "info" };
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    fmt()
        .with_env_filter(filter)
        .with_target(false)
        .without_time()
        .init();
}

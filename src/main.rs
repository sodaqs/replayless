mod cli;
mod compress;
mod config;
mod drive;
mod encode;
mod manifest;
mod probe;
mod scan;

use std::path::Path;

use anyhow::Result;
use clap::Parser;

use crate::cli::{Cli, Command};
use crate::config::Config;

fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    // Load .env into the process environment (Drive auth lives there).
    let _ = dotenvy::dotenv();

    let cfg = Config::load(cli.config.as_deref())?;

    match cli.command {
        Command::Scan => scan::run(&cfg)?,
        Command::Auth => drive::auth::run(Path::new(".env"))?,
        Command::Compress(args) => {
            let overrides = compress::Overrides {
                codec: args.codec,
                cq: args.cq,
                maxrate: args.maxrate,
                fps_cap: args.fps_cap,
                scale: args.scale,
                jobs: args.jobs,
                dry_run: args.dry_run,
                limit: args.limit,
            };
            compress::run(&cfg, &overrides)?;
        }
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

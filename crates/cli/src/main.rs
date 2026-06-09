mod cli;
mod ui;

use std::path::Path;

use anyhow::Result;
use clap::Parser;

use crate::cli::{Cli, Command};
use vu_core::config::Config;
use vu_core::progress::CancelToken;

fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    // Load .env into the process environment (Drive auth lives there).
    let _ = dotenvy::dotenv();

    let cfg = Config::load(cli.config.as_deref())?;

    match cli.command {
        Command::Scan => vu_core::scan::run(&cfg)?,
        Command::Auth => vu_drive::auth::run(Path::new(".env"))?,
        Command::Upload(args) => {
            let opts = vu_drive::Options {
                dry_run: args.dry_run,
                limit: args.limit,
            };
            let mut sink = ui::CliSink::new();
            let cancel = CancelToken::new();
            vu_drive::run(&cfg, &opts, &mut sink, &cancel)?;
        }
        Command::Compress(args) => {
            let overrides = vu_core::compress::Overrides {
                codec: args.codec,
                cq: args.cq,
                maxrate: args.maxrate,
                fps_cap: args.fps_cap,
                scale: args.scale,
                jobs: args.jobs,
                dry_run: args.dry_run,
                limit: args.limit,
            };
            let mut sink = ui::CliSink::new();
            let cancel = CancelToken::new();
            vu_core::compress::run(&cfg, &overrides, &mut sink, &cancel)?;
        }
    }
    Ok(())
}

fn init_tracing(verbose: bool) {
    use tracing_subscriber::{EnvFilter, fmt};

    let default = if verbose { "debug" } else { "info" };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    fmt()
        .with_env_filter(filter)
        .with_target(false)
        .without_time()
        .init();
}

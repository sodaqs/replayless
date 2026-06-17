mod cli;
mod ui;

use anyhow::{Result, bail};
use clap::Parser;

use crate::cli::{Cli, Command};
use vu_core::config::Config;
use vu_core::progress::{CancelToken, ProgressSink};
use vu_core::tooling::{self, ToolStatus};

fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    match cli.command {
        Command::Setup => {
            let mut sink = ui::CliSink::new();
            ensure_ffmpeg_ready(&mut sink)?;
            println!("ffmpeg is ready.");
        }
        Command::Scan => {
            let cfg = Config::load(cli.config.as_deref())?;
            vu_core::scan::run(&cfg)?;
        }
        Command::Compress(args) => {
            let cfg = Config::load(cli.config.as_deref())?;
            let mut sink = ui::CliSink::new();
            // Pre-flight: compression shells out to ffmpeg, so make sure it's
            // present first — installing it via winget when it isn't. A dry run
            // only prints the planned commands, so keep it side-effect-free.
            if !args.dry_run {
                ensure_ffmpeg_ready(&mut sink)?;
            }
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
            let cancel = CancelToken::new();
            vu_core::compress::run(&cfg, &overrides, &mut sink, &cancel)?;
        }
    }
    Ok(())
}

/// Ensure ffmpeg/ffprobe are usable, installing them via winget if missing.
/// Errors with actionable guidance when they're still unavailable afterward, so
/// we never start a doomed compress run.
fn ensure_ffmpeg_ready(sink: &mut dyn ProgressSink) -> Result<()> {
    if tooling::ensure_ffmpeg(sink)? == ToolStatus::Missing {
        bail!(
            "ffmpeg/ffprobe are required but were not found and could not be \
             installed automatically.\n\
             Install them manually with:\n    \
             winget install --id Gyan.FFmpeg -e\n\
             then re-run."
        );
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

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// Compress NVIDIA replay videos with NVENC and upload them to Google Drive.
#[derive(Parser, Debug)]
#[command(name = "video-uploader", version, about)]
pub struct Cli {
    /// Config file to load (default: ./config.toml if present, else built-in defaults).
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,

    /// Verbose (debug) logging.
    #[arg(short, long, global = true)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// List source videos grouped by game and print totals (read-only).
    Scan,
}

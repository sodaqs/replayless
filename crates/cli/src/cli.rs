use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// Compress NVIDIA replay videos with NVENC.
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
    /// Transcode pending videos into the output dir (resumable).
    Compress(CompressArgs),
    /// Check for ffmpeg/ffprobe and install them via winget if missing.
    Setup,
}

#[derive(Args, Debug)]
pub struct CompressArgs {
    /// Codec override: hevc (default) or av1.
    #[arg(long)]
    pub codec: Option<String>,

    /// Quality override (lower = better/bigger; sane 28-34).
    #[arg(long)]
    pub cq: Option<u32>,

    /// Bitrate ceiling override, e.g. 12M.
    #[arg(long)]
    pub maxrate: Option<String>,

    /// Frame-rate cap override (>this is downsampled; 0 = off).
    #[arg(long)]
    pub fps_cap: Option<u32>,

    /// Downscale, e.g. 1920x1080.
    #[arg(long)]
    pub scale: Option<String>,

    /// Concurrent NVENC sessions.
    #[arg(long)]
    pub jobs: Option<usize>,

    /// Print the planned ffmpeg commands without encoding.
    #[arg(long)]
    pub dry_run: bool,

    /// Only process the first N (largest) pending videos — handy for testing.
    #[arg(long)]
    pub limit: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_command_tree_is_valid() {
        // clap's own lint: catches conflicting flags, bad arg defs, and that
        // every subcommand (incl. `setup`) is wired up correctly.
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_setup_subcommand() {
        let cli = Cli::parse_from(["video-uploader", "setup"]);
        assert!(matches!(cli.command, Command::Setup));
    }
}

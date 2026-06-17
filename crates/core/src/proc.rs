//! Spawn child processes without flashing a console window on Windows.
//!
//! The GUI links as a `windows`-subsystem binary, so any console-subsystem
//! child (`ffmpeg`, `ffprobe`, `winget`) would otherwise pop a brief black
//! console window every time it's launched. `CREATE_NO_WINDOW` suppresses that.
//! On the CLI (itself a console binary) the flag is harmless: every child's
//! stdio is piped or nulled, so the child never needs a console of its own.

use std::process::Command;

/// `CREATE_NO_WINDOW`: run the child detached from any console window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Like [`Command::new`], but on Windows the child process won't open a console
/// window. On other platforms this is exactly `Command::new`.
pub fn command(program: &str) -> Command {
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;

    #[test]
    fn builds_command_for_program() {
        // The console-suppression flag is Windows-only and not observable via
        // the public API, but the helper must still build a runnable command
        // for the requested program on every platform.
        let cmd = command("ffprobe");
        assert_eq!(cmd.get_program(), OsStr::new("ffprobe"));
    }
}

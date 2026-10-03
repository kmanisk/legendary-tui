//! Foreground child execution with RAII terminal safety.
//!
//! Callers suspend the Ratatui terminal first (leave alternate screen,
//! restore cooked mode). The guard restores everything on Drop, so Ctrl+C,
//! child failure, or panic can never leave the terminal in raw mode.

use std::io::Write;
use std::process::Command;

use crossterm::{
    cursor::Show,
    event::DisableMouseCapture,
    execute,
    terminal::{disable_raw_mode, LeaveAlternateScreen},
};

/// Restores the terminal when dropped. Create before running Legendary.
pub struct SuspendGuard {
    disarmed: bool,
}

impl SuspendGuard {
    pub fn suspend() -> Result<Self, String> {
        Self::restore_now().map(|_| Self { disarmed: false })
    }

    fn restore_now() -> Result<(), String> {
        let mut out = std::io::stdout();
        disable_raw_mode().map_err(|e| format!("raw mode off: {e}"))?;
        execute!(out, LeaveAlternateScreen, DisableMouseCapture, Show)
            .map_err(|e| format!("leave alt screen: {e}"))?;
        out.flush().ok();
        Ok(())
    }

    /// Terminal is already restored by the main loop; skip double work.
    pub fn disarm(mut self) {
        self.disarmed = true;
    }
}

impl Drop for SuspendGuard {
    fn drop(&mut self) {
        if !self.disarmed {
            let _ = Self::restore_now();
        }
    }
}

/// Run `argv[0]` with inherited stdio (Legendary's own prompts/progress).
/// Returns exit success. Terminal must already be suspended.
pub fn run_foreground(argv: &[&str]) -> bool {
    if argv.is_empty() {
        return false;
    }
    Command::new(argv[0])
        .args(&argv[1..])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

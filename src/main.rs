//! egs — native Epic Games library TUI (legendary backend, Alt+G integration).

mod app;
mod cache;
mod config;
mod filesystem;
mod input;
mod install;
mod launch;
mod legendary;
mod metadata;
mod models;
mod prefix;
mod process;
mod proton;
mod rofi;
mod theme;
mod ui;

use std::io::Write;
use std::time::Duration;

use crossterm::{
    cursor::Hide,
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};

use crate::app::App;
use crate::input::Intent;

fn enter_tui(
) -> Result<ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>, String> {
    enable_raw_mode().map_err(|e| format!("raw mode: {e}"))?;
    let mut out = std::io::stdout();
    execute!(out, EnterAlternateScreen, EnableMouseCapture, Hide)
        .map_err(|e| format!("alt screen: {e}"))?;
    let backend = ratatui::backend::CrosstermBackend::new(out);
    ratatui::Terminal::new(backend).map_err(|e| format!("terminal: {e}"))
}

fn restore_terminal() {
    let mut out = std::io::stdout();
    let _ = disable_raw_mode();
    let _ = execute!(
        out,
        LeaveAlternateScreen,
        DisableMouseCapture,
        crossterm::cursor::Show
    );
    let _ = out.flush();
}

fn to_intent(key: crossterm::event::KeyEvent, searching: bool, in_input: bool) -> Option<Intent> {
    if key.kind != KeyEventKind::Press {
        return None;
    }
    // Ctrl+C always quits cleanly.
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c' | 'C'))
    {
        return Some(Intent::Quit);
    }
    // Ctrl+d / Ctrl+u paging.
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('d') | KeyCode::Char('D') => Some(Intent::PageDown),
            KeyCode::Char('u') | KeyCode::Char('U') => Some(Intent::PageUp),
            _ => None,
        };
    }
    if searching || in_input {
        return match key.code {
            KeyCode::Esc => Some(Intent::Cancel),
            KeyCode::Enter => Some(Intent::Enter),
            KeyCode::Backspace => Some(Intent::Backspace),
            KeyCode::Up => Some(Intent::Up),
            KeyCode::Down => Some(Intent::Down),
            KeyCode::PageUp => Some(Intent::PageUp),
            KeyCode::PageDown => Some(Intent::PageDown),
            KeyCode::Left => Some(Intent::Left),
            KeyCode::Right => Some(Intent::Right),
            KeyCode::Char(c) => Some(Intent::Char(c)),
            _ => None,
        };
    }
    if key.modifiers.contains(KeyModifiers::SHIFT) {
        match key.code {
            KeyCode::Char('j' | 'J') => return Some(Intent::DetailScrollDown),
            KeyCode::Char('k' | 'K') => return Some(Intent::DetailScrollUp),
            _ => {}
        }
    }
    match key.code {
        KeyCode::Char('J') => Some(Intent::DetailScrollDown),
        KeyCode::Char('K') => Some(Intent::DetailScrollUp),
        KeyCode::Char('j') | KeyCode::Down => Some(Intent::Down),
        KeyCode::Char('k') | KeyCode::Up => Some(Intent::Up),
        KeyCode::Char('h') | KeyCode::Left => Some(Intent::Left),
        KeyCode::Char('l') | KeyCode::Right => Some(Intent::Right),
        KeyCode::PageDown => Some(Intent::PageDown),
        KeyCode::PageUp => Some(Intent::PageUp),
        KeyCode::Char('g') => Some(Intent::First),
        KeyCode::Char('G') => Some(Intent::Last),
        KeyCode::Tab => Some(Intent::ToggleSelect),
        KeyCode::Char('f') | KeyCode::Char('F') => Some(Intent::FilterCycle),
        KeyCode::Enter | KeyCode::Char(' ') => Some(Intent::Enter),
        KeyCode::Char('/') => Some(Intent::Search),
        KeyCode::Char('r') => Some(Intent::Refresh),
        KeyCode::Char('u') => Some(Intent::Update),
        KeyCode::Char('d') => Some(Intent::DeleteMenu),
        KeyCode::Char('s') => Some(Intent::Settings),
        KeyCode::Char('?') => Some(Intent::Help),
        KeyCode::Char('q') => Some(Intent::Quit),
        KeyCode::Char('y') | KeyCode::Char('Y') => Some(Intent::ConfirmYes),
        KeyCode::Char('n') | KeyCode::Char('N') => Some(Intent::ConfirmNo),
        KeyCode::Esc => Some(Intent::Cancel),
        _ => None,
    }
}

fn main() {
    // Restore the terminal even on panic.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_hook(info);
    }));

    if let Err(e) = run() {
        restore_terminal();
        eprintln!("egs: {e}");
        std::process::exit(1);
    }
    restore_terminal();
}

fn ensure_legendary_installed() -> Result<(), String> {
    if std::process::Command::new("legendary")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return Ok(());
    }

    let os_release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let mut distro_id = String::new();
    let mut distro_like = String::new();
    let mut distro_name = String::new();

    for line in os_release.lines() {
        if let Some(v) = line.strip_prefix("ID=") {
            distro_id = v.trim_matches('"').to_lowercase();
        } else if let Some(v) = line.strip_prefix("ID_LIKE=") {
            distro_like = v.trim_matches('"').to_lowercase();
        } else if let Some(v) = line.strip_prefix("NAME=") {
            distro_name = v.trim_matches('"').to_string();
        }
    }

    if distro_name.is_empty() {
        distro_name = "Linux".into();
    }

    let has_cmd = |cmd: &str| {
        std::process::Command::new("which")
            .arg(cmd)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };

    let install_cmd = if distro_id == "cachyos"
        || distro_id == "arch"
        || distro_id == "manjaro"
        || distro_id == "endeavouros"
        || distro_like.contains("arch")
    {
        if has_cmd("paru") {
            "paru -S --needed legendary"
        } else if has_cmd("yay") {
            "yay -S --needed legendary"
        } else {
            "sudo pacman -S --needed legendary"
        }
    } else if distro_id == "fedora" || distro_id == "rhel" || distro_like.contains("fedora") {
        "sudo dnf install -y legendary"
    } else if distro_id == "ubuntu"
        || distro_id == "debian"
        || distro_id == "pop"
        || distro_id == "mint"
        || distro_like.contains("debian")
        || distro_like.contains("ubuntu")
    {
        if has_cmd("pipx") {
            "pipx install legendary-gl"
        } else {
            "sudo apt update && sudo apt install -y pipx && pipx install legendary-gl"
        }
    } else if distro_id == "opensuse" || distro_like.contains("suse") {
        "sudo zypper install -y legendary"
    } else if distro_id == "void" {
        "sudo xbps-install -Sy legendary"
    } else if distro_id == "alpine" {
        "sudo apk add legendary"
    } else if has_cmd("pipx") {
        "pipx install legendary-gl"
    } else {
        "python3 -m pip install --user legendary-gl"
    };

    println!("\x1b[1;33m[!] legendary (Epic Games CLI) is not installed.\x1b[0m");
    println!("Detected distribution: \x1b[1;36m{}\x1b[0m", distro_name);
    println!(
        "Recommended install command:\n  \x1b[1;32m{}\x1b[0m\n",
        install_cmd
    );
    print!("Would you like to install legendary now? [Y/n]: ");
    let _ = std::io::stdout().flush();

    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer).is_err() {
        return Err("Failed to read user input".into());
    }
    let trimmed = answer.trim().to_lowercase();
    if !trimmed.is_empty() && trimmed != "y" && trimmed != "yes" {
        return Err("Installation cancelled by user. 'legendary' is required to run egs.".into());
    }

    println!("Running: {}", install_cmd);
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(install_cmd)
        .status()
        .map_err(|e| format!("Failed to run install command: {e}"))?;

    if !status.success() {
        return Err(format!("Install command failed with status: {status}"));
    }

    if !std::process::Command::new("legendary")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        return Err("Installed legendary, but 'legendary' is still not found in PATH.".into());
    }

    println!("\x1b[1;32m[✓] legendary successfully installed! Launching egs...\x1b[0m");
    std::thread::sleep(std::time::Duration::from_millis(600));
    Ok(())
}

fn run() -> Result<(), String> {
    ensure_legendary_installed()?;

    let mut app = App::new()?;
    let mut term = enter_tui()?;

    loop {
        // Poll ongoing background tasks (metadata fetch, active install progress)
        if app.poll_tick() {
            app.dirty = true;
        }

        if app.dirty {
            term.draw(|f| ui::draw(f, &app))
                .map_err(|e| format!("draw: {e}"))?;
            app.dirty = false;
        }
        if !event::poll(Duration::from_millis(100)).map_err(|e| format!("poll: {e}"))? {
            continue;
        }
        let Event::Key(key) = event::read().map_err(|e| format!("read: {e}"))? else {
            continue;
        };
        let in_input = app.in_input_mode();
        let Some(intent) = to_intent(key, app.searching, in_input) else {
            continue;
        };
        if matches!(intent, Intent::Quit) && !app.in_install_mode() {
            break;
        }
        app.handle(intent);
        app.dirty = true;
        if app.suspended {
            // A foreground child ran: re-enter alt screen + raw mode exactly once.
            enable_raw_mode().map_err(|e| format!("raw mode: {e}"))?;
            {
                let mut out = std::io::stdout();
                execute!(out, EnterAlternateScreen, EnableMouseCapture, Hide)
                    .map_err(|e| format!("alt screen: {e}"))?;
            }
            app.suspended = false;
            term.clear().map_err(|e| format!("clear: {e}"))?;
        }
    }
    Ok(())
}

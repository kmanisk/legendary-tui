# legendary-tui (`egs`)

[![Rust](https://img.shields.io/badge/rust-1.80%2B-orange.svg)](https://www.rust-lang.org)
[![AUR package](https://img.shields.io/aur/version/legendary-tui)](https://aur.archlinux.org/packages/legendary-tui)
[![Interface](https://img.shields.io/badge/interface-ratatui-blue.svg)](https://github.com/ratatui/ratatui)

A lightweight, keyboard-driven Ratatui TUI frontend for the [Legendary](https://github.com/derrod/legendary) Epic Games Store CLI, designed for Linux gaming desktops and handhelds.

Installed executable: **`egs`**

---

## Features

### Legendary Core Operations
- **Library Browsing & Filtering**: Instant fuzzy search, sorting, and cycling views between *All*, *Installed*, and *Available* games.
- **Background Downloads & Queueing**: Multi-game queue with real-time percentage, transfer speed, downloaded bytes, ETA, and cancellation support.
- **Cloud Saves Management**:
  - `[l]` List cloud saves and manifest revisions.
  - `[s]` Two-way cloud synchronization (`sync-saves`).
  - `[d]` Direct cloud saves download (`download-saves`).
- **File Verification**: Manifest validation (`legendary verify`) with live percentage, checked file counter, speed, and corrupted file diagnostics.
- **Import Existing Games**: Interactive guided directory selection (`fzf` assisted) for importing pre-existing Epic game installations (`legendary import`).
- **Cache & Temporary Cleanup**: Safe maintenance action to clean chunk caches and stale manifests (`legendary cleanup`).
- **Account & Session Management**: Account overview (`legendary status --json`), interactive re-authentication, and secure credential purge.

### Linux Gaming & Compositor Integration
- **Proton & Wine Discovery**: Automatic discovery of installed Proton-GE, Proton Experimental, Proton CachyOS, and system Wine runners.
- **Per-Game Tuning**:
  - Dedicated prefix root isolation (`~/.local/share/egs/prefixes/<appid>/`).
  - Per-game or global Proton version selection.
  - MangoHud HUD toggle.
  - Feral GameMode optimization toggle.
  - Lossless Scaling Frame Generation (LSFG) multiplier settings.
- **ProtonDB Integration**: Compatibility tier badge (Platinum, Gold, Silver, Bronze, Borked) with fast background queries and persistent cache.
- **Rofi Desktop Launcher**: Instant integration with `Alt+G` Rofi game menus (`~/.config/rofi/epic-games.list`).
- **Live Theme Synchronization**: Adapts dynamically to system palette changes (watches theme files and handles `SIGUSR2`).

---

## Keyboard Controls

### Library Navigation
| Key | Action |
| :--- | :--- |
| `j` / `k` or `↓` / `↑` | Move cursor down / up |
| `J` / `K` (or `Shift+j/k`) | Scroll game description & details pane |
| `Ctrl+d` / `Ctrl+u` | Page down / Page up |
| `g` / `G` | Jump to first / last game in list |
| `Tab` | Toggle game multi-selection |
| `f` | Cycle filter (*All* → *Installed* → *Available*) |
| `/` | Start fuzzy search (`Esc` to cancel, `Enter` to confirm) |
| `r` | Refresh library from Epic Games Store |
| `s` | Open Global Settings & Maintenance |
| `?` | Show Help modal |
| `q` | Quit application |

### Game Actions
| Key | Context | Action |
| :--- | :--- | :--- |
| `Enter` | Installed | Open Launch Options & Settings menu |
| `Enter` | Available | Start game installation |
| `c` | Any / Installed | Open **Cloud Saves** menu |
| `v` | Installed | **Verify game files** against manifest |
| `i` | Library / Settings | **Import existing game** installation |
| `u` | Installed | Check for updates / run repair |
| `d` | Installed | Delete game (options for keeping or removing prefix) |
| `x` | Active download/verify | Cancel download, dequeue, or cancel verification |

### Cloud Saves Menu
| Key | Action |
| :--- | :--- |
| `l` | List remote save manifests |
| `s` | Synchronize saves (two-way) |
| `d` | Download remote saves (replaces local) |
| `Esc` | Return to library |

---

## Installation

### Arch Linux / CachyOS (AUR)
```bash
# Release package:
yay -S legendary-tui
# Or development git package:
yay -S legendary-tui-git
```

### From Source (Cargo)
Ensure Rust and Cargo are installed:
```bash
git clone https://github.com/kmanisk/legendary-tui.git
cd legendary-tui
cargo build --release
install -Dm755 target/release/egs ~/.local/bin/egs
```

### Dependencies
- **`legendary`** (v0.21+ recommended)
- **`fzf`** (optional, enables interactive directory browsing when importing or moving games)

---

## Architecture & Safety Guarantees

- **No Daemons**: `egs` exits completely when closed; no lingering background processes or persistent system services.
- **Fail-Safe Destructive Operations**: All delete, move, sync overwrite, and logout operations require explicit modal confirmation.
- **Prefix Isolation**: Dedicated Wine prefixes ensure game compatdata stays clean and unpolluted.

//! Application state: library, selection, search, dialogs, mutations.
//! All destructive paths go through explicit confirm screens and fail closed.

use std::collections::{HashMap, HashSet};

use crate::input::Intent;
use crate::install::{ActiveInstall, InstallProgress};
use crate::models::{full_search_score, Game, GameDetails};
use crate::{cache, config, legendary, metadata, prefix, process, proton, rofi};

/// Forward operation chosen from a confirm/input dialog.
#[derive(Clone, Debug)]
pub(crate) enum Op {
    Play(String),
    Update(String),
    DeleteGame(String),
    DeleteGamePrefix(String),
    DeleteBatch(Vec<String>),
    RemoveEntry(String),
    AddEntry(String),
    Details(String),
    SetDefaultPath,
    SetPrefixPath,
    SetDefaultProton,
    SetDefaultGameMode,
    SetDefaultMangoHud,
    SetLsfg,
    SetPerGamePath(String),
    MoveGame(String),
    DoMoveGame { app: String, dest: String },
    SetPerGamePrefix(String),
    SetPerGameLaunchArgs(String),
    SetPerGameProton(String),
    SetPerGameGameMode(String),
    SetPerGameMangoHud(String),
    SetPerGameLsfg(String),
    StartInstall(String),
    StartInstallBatch(Vec<String>),
    CleanStale,
    Back,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum RowItem {
    Header(&'static str, usize),
    Game(usize),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum MenuKind {
    Installed(String),
    Settings,
    ProtonSelect { app: Option<String> },
    DeleteConfirm(String),
    BatchInstalled(Vec<String>),
}

#[derive(Clone, Debug)]
pub(crate) struct Menu {
    pub(crate) title: String,
    pub(crate) items: Vec<(String, Op)>,
    pub(crate) idx: usize,
    pub(crate) kind: MenuKind,
}

#[derive(Clone, Debug)]
pub(crate) enum Mode {
    Library,
    Install(InstallProgress),
    Menu(Menu),
    Confirm { lines: Vec<String>, op: Op },
    Input { title: String, buf: String, op: Op },
    Details(String),
    Help,
}

pub struct App {
    pub(crate) games: Vec<Game>,
    /// Categorized rows (headers and game indices).
    pub(crate) filtered: Vec<RowItem>,
    pub selected: usize,
    pub detail_scroll: u16,
    pub search: String,
    pub searching: bool,
    mode: Mode,
    pub status: String,
    pub cfg: config::Config,
    prefix_sizes: HashMap<String, u64>,
    rofi_entries: Vec<rofi::Entry>,
    pub dirty: bool,
    /// Set when a foreground child ran (TUI left alt screen); main loop re-enters.
    pub(crate) suspended: bool,
    pub(crate) filter: Filter,
    /// Deterministic `gg` state: first `g` arms, second executes, anything else clears.
    pending_g: bool,
    pub theme: crate::theme::Theme,
    pub(crate) metadata_mgr: metadata::MetadataManager,
    pub(crate) active_install: Option<ActiveInstall>,
    pub(crate) selected_details: Option<GameDetails>,
    pub(crate) selected_games: HashSet<String>,
    pub(crate) install_queue: Vec<String>,
    pub(crate) refreshing: bool,
    pub(crate) refresh_rx: Option<std::sync::mpsc::Receiver<LibraryResult>>,
}

type LibraryResult = Result<Vec<(String, String)>, String>;

/// Library filter, cycled with `f`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Filter {
    All,
    Installed,
    Available,
}

impl Filter {
    fn next(self) -> Self {
        match self {
            Filter::All => Filter::Installed,
            Filter::Installed => Filter::Available,
            Filter::Available => Filter::All,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Filter::All => "All",
            Filter::Installed => "Installed",
            Filter::Available => "Not installed",
        }
    }
}

impl App {
    pub fn new() -> Result<Self, String> {
        let cfg = config::load();
        // Cache-first for instant startup; refresh only on demand.
        let cached = cache::load();
        let lib: Vec<(String, String)> = match cached {
            Some(games) => games.into_iter().map(|g| (g.app_name, g.title)).collect(),
            None => Self::fetch_library()?,
        };
        let mut app = Self {
            games: Vec::new(),
            filtered: Vec::new(),
            selected: 0,
            detail_scroll: 0,
            search: String::new(),
            searching: false,
            mode: Mode::Library,
            status: String::new(),
            cfg,
            prefix_sizes: HashMap::new(),
            rofi_entries: Vec::new(),
            dirty: true,
            suspended: false,
            filter: Filter::All,
            pending_g: false,
            theme: crate::theme::load(),
            metadata_mgr: metadata::MetadataManager::new(),
            active_install: None,
            selected_details: None,
            selected_games: HashSet::new(),
            install_queue: Vec::new(),
            refreshing: false,
            refresh_rx: None,
        };
        app.set_library(lib);
        app.refresh_installed()?;
        app.update_selected_details();
        Ok(app)
    }

    fn fetch_library() -> Result<Vec<(String, String)>, String> {
        let lib = legendary::library()?;
        let cached: Vec<cache::CachedGame> = lib
            .iter()
            .map(|(id, title)| cache::CachedGame {
                app_name: id.clone(),
                title: title.clone(),
            })
            .collect();
        // Best effort: a failed cache write must not break startup.
        let _ = cache::save(&cached);
        Ok(lib)
    }

    fn set_library(&mut self, lib: Vec<(String, String)>) {
        self.games = lib
            .into_iter()
            .map(|(app_name, title)| Game {
                app_name,
                title,
                installed: false,
                version: None,
                install_path: None,
            })
            .collect();
        self.apply_filter();
    }

    /// Re-read installed state + Alt+G entries. Preserves selection.
    pub fn refresh_installed(&mut self) -> Result<(), String> {
        let sel_id = self.current().map(|g| g.app_name.clone());
        let map = legendary::installed_map()?;
        for g in &mut self.games {
            match map.get(&g.app_name) {
                Some(info) => {
                    g.installed = true;
                    g.version = Some(info.version.clone());
                    g.install_path = (!info.path.is_empty()).then(|| info.path.clone());
                }
                None => {
                    g.installed = false;
                    g.version = None;
                    g.install_path = None;
                }
            }
        }
        self.rofi_entries = rofi::entries();
        self.apply_filter();
        if let Some(id) = sel_id {
            if let Some(pos) = self.filtered.iter().position(|r| match r {
                RowItem::Game(i) => self.games[*i].app_name == id,
                _ => false,
            }) {
                self.selected = pos;
            }
        }
        self.update_selected_details();
        Ok(())
    }

    pub fn poll_tick(&mut self) -> bool {
        let mut updated = false;

        // 1. Poll metadata background fetch updates
        if self.metadata_mgr.poll_updates() {
            self.update_selected_details();
            updated = true;
        }

        // 2. Poll ongoing install process
        if let Some(inst) = &mut self.active_install {
            match inst.poll() {
                Ok(Some(())) => {
                    let app_id = inst.app_name.clone();
                    let title = inst.title.clone();
                    self.active_install = None;
                    let _ = self.refresh_installed();
                    let _ = rofi::ensure(&title, &app_id);
                    self.selected_games.remove(&app_id);

                    if let Some(next_app) = self.install_queue.pop() {
                        let next_title = self.title_of(&next_app);
                        self.say(format!("Installed {title}. Starting {next_title}..."));
                        self.start_in_app_install(&next_app);
                    } else {
                        self.say(format!("Installed {title} successfully."));
                        self.mode = Mode::Library;
                    }
                    updated = true;
                }
                Ok(None) => {
                    self.mode = Mode::Install(inst.progress.clone());
                    updated = true;
                }
                Err(e) => {
                    let title = inst.title.clone();
                    self.active_install = None;
                    self.install_queue.clear();
                    self.say(format!("Install of {title} stopped: {e}"));
                    self.mode = Mode::Library;
                    let _ = self.refresh_installed();
                    updated = true;
                }
            }
        }

        // 3. Poll background library refresh
        if let Some(rx) = &self.refresh_rx {
            if let Ok(res) = rx.try_recv() {
                self.refreshing = false;
                self.refresh_rx = None;
                match res {
                    Ok(lib) => {
                        let cached: Vec<cache::CachedGame> = lib
                            .iter()
                            .map(|(id, title)| cache::CachedGame {
                                app_name: id.clone(),
                                title: title.clone(),
                            })
                            .collect();
                        let _ = cache::save(&cached);
                        self.set_library(lib);
                        self.theme = crate::theme::load();
                        if let Err(e) = self.refresh_installed() {
                            self.say(format!("installed refresh failed: {e}"));
                        } else {
                            self.say("Library refreshed.");
                        }
                    }
                    Err(e) => {
                        self.say(format!("refresh failed (kept cache): {e}"));
                    }
                }
                updated = true;
            }
        }

        updated
    }

    pub fn in_input_mode(&self) -> bool {
        matches!(self.mode, Mode::Input { .. })
    }

    pub fn in_install_mode(&self) -> bool {
        self.active_install.is_some() || matches!(self.mode, Mode::Install(_))
    }

    pub fn update_selected_details(&mut self) {
        if let Some(g) = self.current().cloned() {
            let mut d = self.metadata_mgr.get_or_load(&g);
            if let Some(details) = &mut d {
                if g.installed {
                    details.installed = true;
                    if details.installed_size.is_none() {
                        if let Some(p) = &g.install_path {
                            let n = prefix::dir_size_bytes(std::path::Path::new(p));
                            details.installed_size = Some(n);
                        }
                    }
                }
            }
            self.selected_details = d;
        } else {
            self.selected_details = None;
        }
    }

    fn apply_filter(&mut self) {
        let mut scored: Vec<(u8, usize)> = self
            .games
            .iter()
            .enumerate()
            .filter(|(_, g)| match self.filter {
                Filter::All => true,
                Filter::Installed => g.installed,
                Filter::Available => !g.installed,
            })
            .filter_map(|(i, g)| {
                if self.search.trim().is_empty() {
                    Some((1, i))
                } else {
                    let det = self.metadata_mgr.get_or_load(g);
                    full_search_score(g, det.as_ref(), &self.search).map(|s| (s, i))
                }
            })
            .collect();

        if self.search.trim().is_empty() {
            scored.sort_by(|a, b| self.games[a.1].title.cmp(&self.games[b.1].title));
        } else {
            scored.sort_by(|a, b| {
                b.0.cmp(&a.0)
                    .then_with(|| self.games[a.1].title.cmp(&self.games[b.1].title))
            });
        }

        let mut installed_rows = Vec::new();
        let mut library_rows = Vec::new();

        for (_, idx) in scored {
            if self.games[idx].installed {
                installed_rows.push(idx);
            } else {
                library_rows.push(idx);
            }
        }

        let mut rows = Vec::new();
        if !installed_rows.is_empty() {
            rows.push(RowItem::Header("Installed Games", installed_rows.len()));
            for idx in installed_rows {
                rows.push(RowItem::Game(idx));
            }
        }
        if !library_rows.is_empty() {
            rows.push(RowItem::Header("Library", library_rows.len()));
            for idx in library_rows {
                rows.push(RowItem::Game(idx));
            }
        }

        self.filtered = rows;
        self.ensure_valid_selection(self.selected);
        self.detail_scroll = 0;
    }

    fn ensure_valid_selection(&mut self, preferred: usize) {
        if self.filtered.is_empty() {
            self.selected = 0;
            return;
        }
        let max_idx = self.filtered.len().saturating_sub(1);
        let preferred = preferred.min(max_idx);

        if let Some(RowItem::Game(_)) = self.filtered.get(preferred) {
            self.selected = preferred;
            return;
        }
        for idx in preferred..self.filtered.len() {
            if matches!(self.filtered.get(idx), Some(RowItem::Game(_))) {
                self.selected = idx;
                return;
            }
        }
        for idx in (0..preferred).rev() {
            if matches!(self.filtered.get(idx), Some(RowItem::Game(_))) {
                self.selected = idx;
                return;
            }
        }
        self.selected = 0;
    }

    fn jump_first(&mut self) {
        if let Some(pos) = self
            .filtered
            .iter()
            .position(|r| matches!(r, RowItem::Game(_)))
        {
            self.selected = pos;
        } else {
            self.selected = 0;
        }
        self.detail_scroll = 0;
        self.update_selected_details();
    }

    fn jump_last(&mut self) {
        if let Some(pos) = self
            .filtered
            .iter()
            .rposition(|r| matches!(r, RowItem::Game(_)))
        {
            self.selected = pos;
        } else {
            self.selected = 0;
        }
        self.detail_scroll = 0;
        self.update_selected_details();
    }

    fn cycle_filter(&mut self) {
        self.filter = self.filter.next();
        self.apply_filter();
        self.update_selected_details();
    }

    pub fn status_line(&self) -> String {
        if let Mode::Install(prog) = &self.mode {
            return format!(
                "Installing: {} | {:.1}% | {} | ETA: {}",
                prog.title, prog.percentage, prog.speed_str, prog.eta_str
            );
        }
        let installed = self.games.iter().filter(|g| g.installed).count();
        let mut total_installed_bytes = 0u64;
        for g in &self.games {
            if g.installed {
                if let Some(details) = self.metadata_mgr.load_from_cache(&g.app_name) {
                    if let Some(s) = details.installed_size {
                        total_installed_bytes = total_installed_bytes.saturating_add(s);
                    }
                }
            }
        }
        let size_info = if total_installed_bytes > 0 {
            format!(" | {}", prefix::fmt_size(total_installed_bytes))
        } else {
            String::new()
        };

        let mut s = format!(
            "{} Games | {} Installed{} | Proton: {} | Filter: {}",
            self.games.len(),
            installed,
            size_info,
            self.cfg.default_proton,
            self.filter.label()
        );
        if !self.status.is_empty() {
            s.push_str("   ");
            s.push_str(&self.status);
        }
        s
    }

    pub fn current(&self) -> Option<&Game> {
        match self.filtered.get(self.selected) {
            Some(RowItem::Game(i)) => self.games.get(*i),
            _ => None,
        }
    }

    pub fn current_details(&self) -> Option<&GameDetails> {
        self.selected_details.as_ref()
    }

    fn say(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
        self.dirty = true;
    }

    pub fn registry_prefix(&self, app: &str) -> String {
        self.rofi_entries
            .iter()
            .find(|e| e.appid == app)
            .map(|e| e.prefix.clone())
            .unwrap_or_default()
    }

    pub fn prefix_path(&self, app: &str) -> Option<std::path::PathBuf> {
        prefix::resolve(app, &self.registry_prefix(app))
    }

    pub fn prefix_size(&mut self, app: &str) -> Option<u64> {
        if let Some(&n) = self.prefix_sizes.get(app) {
            return Some(n);
        }
        let p = self.prefix_path(app)?;
        let n = prefix::dir_size_bytes(&p);
        self.prefix_sizes.insert(app.to_string(), n);
        Some(n)
    }

    fn title_of(&self, app: &str) -> String {
        self.games
            .iter()
            .find(|g| g.app_name == app)
            .map(|g| g.title.clone())
            .unwrap_or_else(|| app.to_string())
    }

    fn resolve_install(&self, app: &str) -> (String, Option<String>, &'static str) {
        if let Some(p) = self.cfg.games.get(app).and_then(|g| g.install_path.clone()) {
            let path = std::path::Path::new(&p);
            let folder = path.file_name().map(|s| s.to_string_lossy().into_owned());
            let base = path
                .parent()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or(p.clone());
            return (base, folder, "per-game setting");
        }
        let global = self.cfg.default_install_path.clone();
        if !global.is_empty() {
            return (global, None, "global default");
        }
        let fb =
            legendary::legendary_default_dir().unwrap_or_else(|| "/mnt/Games/EpicGames".into());
        (fb, None, "legendary fallback")
    }

    // ---- In-App Install ---------------------------------------------------
    fn start_in_app_install(&mut self, app: &str) {
        if self.active_install.is_some() {
            self.say("An installation is already running.");
            return;
        }
        let (base, folder, _) = self.resolve_install(app);
        let title = self.title_of(app);
        let total = self
            .current_details()
            .and_then(|d| d.download_size)
            .unwrap_or(0);
        match ActiveInstall::start(app, &title, &base, folder.as_deref(), total) {
            Ok(inst) => {
                let initial = inst.progress.clone();
                self.active_install = Some(inst);
                self.mode = Mode::Install(initial);
                self.say(format!("Installing {title}..."));
            }
            Err(e) => {
                self.say(format!("Install start failed: {e}"));
            }
        }
    }

    // ---- Launch -----------------------------------------------------------
    pub fn launch_game(&mut self, app_name: &str) {
        let title = self.title_of(app_name);
        let install_path_str = match self
            .games
            .iter()
            .find(|g| g.app_name == app_name)
            .and_then(|g| g.install_path.as_deref())
        {
            Some(p) => p.to_string(),
            None => {
                self.say("Game install path not found.");
                return;
            }
        };
        let install_path = std::path::Path::new(&install_path_str);
        let known_exe = self.current_details().and_then(|d| d.launch_exe.as_deref());
        let game_exe = match crate::launch::resolve_game_exe(install_path, known_exe) {
            Ok(exe) => exe,
            Err(e) => {
                self.mode = Mode::Confirm {
                    lines: vec!["Launch failed.".into(), "".into(), e],
                    op: Op::Back,
                };
                return;
            }
        };

        let proton_choice = self
            .cfg
            .games
            .get(app_name)
            .and_then(|g| g.proton.clone())
            .unwrap_or_else(|| self.cfg.default_proton.clone());

        let prefix_path = if let Some(p) = self
            .cfg
            .games
            .get(app_name)
            .and_then(|g| g.prefix_path.clone())
        {
            let pb = std::path::PathBuf::from(p);
            let _ = crate::filesystem::ensure_dir(&pb);
            pb
        } else {
            match crate::prefix::ensure_prefix(app_name, &self.registry_prefix(app_name)) {
                Ok(p) => p,
                Err(e) => {
                    self.say(format!("Cannot establish prefix: {e}"));
                    return;
                }
            }
        };

        let gamemode = self
            .cfg
            .games
            .get(app_name)
            .and_then(|g| g.gamemode)
            .unwrap_or(self.cfg.default_gamemode);

        let mangohud = self
            .cfg
            .games
            .get(app_name)
            .and_then(|g| g.mangohud)
            .unwrap_or(self.cfg.default_mangohud);

        let lsfg_choice = self
            .cfg
            .games
            .get(app_name)
            .and_then(|g| g.lsfg.clone())
            .unwrap_or_else(|| self.cfg.lsfg_multiplier.clone());

        let extra_args: Vec<String> = self
            .cfg
            .games
            .get(app_name)
            .and_then(|g| g.launch_args.clone())
            .map(|s| s.split_whitespace().map(|w| w.to_string()).collect())
            .unwrap_or_default();

        let launch_cfg = crate::launch::LaunchConfig {
            app_name: app_name.to_string(),
            game_exe,
            working_dir: install_path.to_path_buf(),
            prefix_path,
            proton_choice,
            gamemode,
            mangohud,
            lsfg_choice,
            extra_args,
            env_vars: Vec::new(),
        };

        let mut cmd = match launch_cfg.build_command() {
            Ok(c) => c,
            Err(e) => {
                self.mode = Mode::Confirm {
                    lines: vec!["Launch error:".into(), "".into(), e],
                    op: Op::Back,
                };
                return;
            }
        };

        let log_path = format!("/tmp/egs-launch-{app_name}.log");
        let log_file = match std::fs::File::create(&log_path) {
            Ok(f) => f,
            Err(e) => {
                self.say(format!("Failed to create launch log: {e}"));
                return;
            }
        };
        let log_file_err = match log_file.try_clone() {
            Ok(f) => f,
            Err(e) => {
                self.say(format!("Failed to clone log file handle: {e}"));
                return;
            }
        };

        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::from(log_file))
            .stderr(std::process::Stdio::from(log_file_err));

        match cmd.spawn() {
            Ok(_child) => {
                self.say(format!("Launched {title} in background. Log: {log_path}"));
            }
            Err(e) => {
                self.say(format!("Failed to spawn {title}: {e}"));
            }
        }
    }

    // ---- Multi-Selection (Tab) --------------------------------------------
    pub fn toggle_select(&mut self) {
        if let Some(g) = self.current().cloned() {
            let app_id = g.app_name.clone();
            if self.selected_games.contains(&app_id) {
                self.selected_games.remove(&app_id);
                let count = self.selected_games.len();
                if count == 0 {
                    self.say("Selection cleared.");
                } else {
                    self.say(format!("Deselected {}. {count} selected.", g.title));
                }
            } else {
                let has_installed = self.selected_games.iter().any(|id| {
                    self.games
                        .iter()
                        .find(|gm| &gm.app_name == id)
                        .map(|gm| gm.installed)
                        .unwrap_or(false)
                });
                let has_uninstalled = self.selected_games.iter().any(|id| {
                    self.games
                        .iter()
                        .find(|gm| &gm.app_name == id)
                        .map(|gm| !gm.installed)
                        .unwrap_or(false)
                });

                if has_installed && !g.installed {
                    self.say(
                        "Cannot mix Installed and Available games. Press Esc to clear selection.",
                    );
                    return;
                }
                if has_uninstalled && g.installed {
                    self.say(
                        "Cannot mix Available and Installed games. Press Esc to clear selection.",
                    );
                    return;
                }

                self.selected_games.insert(app_id);
                let count = self.selected_games.len();
                let kind = if g.installed {
                    "Installed"
                } else {
                    "Available"
                };
                self.say(format!(
                    "Selected {count} {kind} game{}.",
                    if count == 1 { "" } else { "s" }
                ));
            }
            self.move_sel(1);
            self.dirty = true;
        }
    }

    // ---- Operations Execution ---------------------------------------------
    fn execute(&mut self, op: Op) {
        match op {
            Op::Play(app) => self.launch_game(&app),
            Op::Details(_) => {}
            Op::Update(app) => self.start_in_app_install(&app),
            Op::MoveGame(app) => {
                if app.is_empty() {
                    self.say("Select an installed game first.");
                    return;
                }
                let title = self.title_of(&app);
                let cur_path = self
                    .games
                    .iter()
                    .find(|g| g.app_name == app)
                    .and_then(|g| g.install_path.clone())
                    .unwrap_or_else(|| "Unknown".into());

                let guard = match process::SuspendGuard::suspend() {
                    Ok(g) => g,
                    Err(e) => {
                        self.say(format!("terminal suspend failed: {e}"));
                        return;
                    }
                };
                let picked =
                    crate::filesystem::pick_directory_fzf(&self.cfg.default_install_path, &title);
                guard.disarm();
                self.suspended = true;

                if let Some(dest) = picked {
                    let dest = dest.trim().to_string();
                    if !dest.is_empty() {
                        self.mode = Mode::Confirm {
                            lines: vec![
                                format!("Move {title}?"),
                                format!("Current path:     {cur_path}"),
                                format!("Destination base: {dest}"),
                                "".into(),
                                "Legendary will move game files and update database.".into(),
                            ],
                            op: Op::DoMoveGame { app, dest },
                        };
                        return;
                    }
                }
                self.say("Move cancelled.");
            }
            Op::DoMoveGame { app, dest } => {
                let title = self.title_of(&app);
                let guard = match process::SuspendGuard::suspend() {
                    Ok(g) => g,
                    Err(e) => {
                        self.say(format!("terminal suspend failed: {e}"));
                        return;
                    }
                };
                let _ = std::fs::create_dir_all(&dest);
                let ok = process::run_foreground(&["legendary", "-y", "move", &app, &dest]);
                guard.disarm();
                self.suspended = true;
                if ok {
                    if let Some(game_cfg) = self.cfg.games.get_mut(&app) {
                        if game_cfg.install_path.is_some() {
                            let mut new_path = std::path::PathBuf::from(&dest);
                            if let Some(g) = self.games.iter().find(|g| g.app_name == app) {
                                if let Some(old_p) = &g.install_path {
                                    if let Some(folder_name) =
                                        std::path::Path::new(old_p).file_name()
                                    {
                                        new_path.push(folder_name);
                                    }
                                }
                            }
                            game_cfg.install_path = Some(new_path.to_string_lossy().into_owned());
                            let _ = config::save(&self.cfg);
                        }
                    }
                    let _ = self.refresh_installed();
                    self.say(format!("{title} moved successfully to {dest}."));
                } else {
                    self.say(format!("Move of {title} failed or was cancelled."));
                }
            }
            Op::DeleteGame(app) => {
                let title = self.title_of(&app);
                let guard = match process::SuspendGuard::suspend() {
                    Ok(g) => g,
                    Err(e) => {
                        self.say(format!("terminal suspend failed: {e}"));
                        return;
                    }
                };
                let ok = process::run_foreground(&["legendary", "uninstall", &app]);
                guard.disarm();
                self.suspended = true;
                if ok {
                    let _ = rofi::remove(&app);
                    self.rofi_entries = rofi::entries();
                    let _ = self.refresh_installed();
                    self.say(format!("{title} uninstalled (prefix kept)."));
                } else {
                    self.say("Uninstall did not complete — nothing removed.");
                }
            }
            Op::DeleteGamePrefix(app) => {
                let title = self.title_of(&app);
                let guard = match process::SuspendGuard::suspend() {
                    Ok(g) => g,
                    Err(e) => {
                        self.say(format!("terminal suspend failed: {e}"));
                        return;
                    }
                };
                let ok = process::run_foreground(&["legendary", "uninstall", &app]);
                guard.disarm();
                self.suspended = true;
                if ok {
                    match self.prefix_path(&app) {
                        Some(p) => match std::fs::remove_dir_all(&p) {
                            Ok(_) => {
                                self.prefix_sizes.remove(&app);
                                self.say(format!("{title} + prefix removed."));
                            }
                            Err(e) => self.say(format!("game gone, prefix kept ({e})")),
                        },
                        None => {
                            self.say("game gone; no confident prefix found, nothing else touched.")
                        }
                    }
                    let _ = rofi::remove(&app);
                    self.rofi_entries = rofi::entries();
                    let _ = self.refresh_installed();
                } else {
                    self.say("Uninstall did not complete — nothing removed.");
                }
            }
            Op::DeleteBatch(apps) => {
                let guard = match process::SuspendGuard::suspend() {
                    Ok(g) => g,
                    Err(e) => {
                        self.say(format!("terminal suspend failed: {e}"));
                        return;
                    }
                };
                let mut count = 0;
                for app in &apps {
                    let ok = process::run_foreground(&["legendary", "uninstall", "-y", app]);
                    if ok {
                        let _ = rofi::remove(app);
                        self.selected_games.remove(app);
                        count += 1;
                    }
                }
                guard.disarm();
                self.suspended = true;
                self.rofi_entries = rofi::entries();
                let _ = self.refresh_installed();
                self.say(format!("Uninstalled {count} of {} games.", apps.len()));
            }
            Op::RemoveEntry(app) => match rofi::remove(&app) {
                Ok(true) => {
                    self.rofi_entries = rofi::entries();
                    self.say("Alt+G entry removed (game kept).");
                }
                Ok(false) => self.say("No Alt+G entry existed."),
                Err(e) => self.say(format!("entry removal failed: {e}")),
            },
            Op::AddEntry(app) => {
                let title = self.title_of(&app);
                match rofi::ensure(&title, &app) {
                    Ok(true) => self.say(format!("Added {title} to Alt+G.")),
                    Ok(false) => self.say("Alt+G entry already present."),
                    Err(e) => self.say(format!("entry add failed: {e}")),
                }
                self.rofi_entries = rofi::entries();
            }
            Op::StartInstall(app) => self.start_in_app_install(&app),
            Op::StartInstallBatch(apps) => {
                if !apps.is_empty() {
                    let first = apps[0].clone();
                    self.install_queue = apps.into_iter().skip(1).rev().collect();
                    self.start_in_app_install(&first);
                }
            }
            Op::SetDefaultPath => {}
            Op::SetPrefixPath => {}
            Op::SetDefaultProton => {
                let protons = proton::discover_protons();
                let mut items = vec![("Auto".to_string(), Op::SetDefaultProton)];
                for p in protons {
                    items.push((p.name.clone(), Op::SetDefaultProton));
                }
                self.mode = Mode::Menu(Menu {
                    title: "Select Default Proton".to_string(),
                    items,
                    idx: 0,
                    kind: MenuKind::ProtonSelect { app: None },
                });
            }
            Op::SetDefaultGameMode => {
                self.cfg.default_gamemode = !self.cfg.default_gamemode;
                let _ = config::save(&self.cfg);
                self.say(format!(
                    "Default GameMode: {}",
                    if self.cfg.default_gamemode {
                        "On"
                    } else {
                        "Off"
                    }
                ));
            }
            Op::SetDefaultMangoHud => {
                self.cfg.default_mangohud = !self.cfg.default_mangohud;
                let _ = config::save(&self.cfg);
                self.say(format!(
                    "Default MangoHud: {}",
                    if self.cfg.default_mangohud {
                        "Yes"
                    } else {
                        "No"
                    }
                ));
            }
            Op::SetLsfg => {
                let current = &self.cfg.lsfg_multiplier;
                let next = match current.as_str() {
                    "Disabled" => "2x",
                    "2x" => "3x",
                    "3x" => "4x",
                    "4x" => "Disabled",
                    _ => "Disabled",
                };
                self.cfg.lsfg_multiplier = next.to_string();
                let _ = config::save(&self.cfg);
                self.say(format!("LSFG Frame Generation: {next}"));
            }
            Op::SetPerGamePath(_) => {}
            Op::SetPerGameProton(app) => {
                let protons = proton::discover_protons();
                let mut items = vec![(
                    "Default (Global)".to_string(),
                    Op::SetPerGameProton(app.clone()),
                )];
                for p in protons {
                    items.push((p.name.clone(), Op::SetPerGameProton(app.clone())));
                }
                self.mode = Mode::Menu(Menu {
                    title: format!("Proton for {}", self.title_of(&app)),
                    items,
                    idx: 0,
                    kind: MenuKind::ProtonSelect {
                        app: Some(app.clone()),
                    },
                });
            }
            Op::SetPerGameGameMode(app) => {
                let cur = self
                    .cfg
                    .games
                    .get(&app)
                    .and_then(|g| g.gamemode)
                    .unwrap_or(self.cfg.default_gamemode);
                self.cfg.games.entry(app.clone()).or_default().gamemode = Some(!cur);
                let _ = config::save(&self.cfg);
                self.say(format!(
                    "GameMode for {}: {}",
                    self.title_of(&app),
                    if !cur { "On" } else { "Off" }
                ));
            }
            Op::SetPerGameMangoHud(app) => {
                let cur = self
                    .cfg
                    .games
                    .get(&app)
                    .and_then(|g| g.mangohud)
                    .unwrap_or(self.cfg.default_mangohud);
                self.cfg.games.entry(app.clone()).or_default().mangohud = Some(!cur);
                let _ = config::save(&self.cfg);
                self.say(format!(
                    "MangoHud for {}: {}",
                    self.title_of(&app),
                    if !cur { "Yes" } else { "No" }
                ));
            }
            Op::SetPerGameLsfg(app) => {
                let cur = self
                    .cfg
                    .games
                    .get(&app)
                    .and_then(|g| g.lsfg.clone())
                    .unwrap_or_else(|| "Disabled".into());
                let next = match cur.as_str() {
                    "Disabled" => "2x",
                    "2x" => "3x",
                    "3x" => "4x",
                    "4x" => "Disabled",
                    _ => "Disabled",
                };
                self.cfg.games.entry(app.clone()).or_default().lsfg = Some(next.to_string());
                let _ = config::save(&self.cfg);
                self.say(format!("LSFG for {}: {next}", self.title_of(&app)));
            }
            Op::CleanStale => {
                let installed: HashSet<String> = self
                    .games
                    .iter()
                    .filter(|g| g.installed)
                    .map(|g| g.app_name.clone())
                    .collect();
                let stale = rofi::stale(&installed);
                if stale.is_empty() {
                    self.say("No stale Alt+G entries.");
                } else {
                    for e in &stale {
                        let _ = rofi::remove(&e.appid);
                    }
                    self.rofi_entries = rofi::entries();
                    let missing_n = rofi::missing(&installed).len();
                    let mut msg = format!("Cleaned {} stale Alt+G entries.", stale.len());
                    if missing_n > 0 {
                        msg.push_str(&format!(
                            " {missing_n} installed games lack entries (Enter on them to add)."
                        ));
                    }
                    self.say(msg);
                }
            }
            Op::SetPerGamePrefix(_) | Op::SetPerGameLaunchArgs(_) => {}
            Op::Back => {}
        }
        self.dirty = true;
    }

    // ---- Menus ------------------------------------------------------------
    fn installed_menu(&self, app: &str) -> Menu {
        let in_rofi = self.rofi_entries.iter().any(|e| e.appid == app);
        let pfx_label = self
            .cfg
            .games
            .get(app)
            .and_then(|g| g.prefix_path.clone())
            .unwrap_or_else(|| {
                self.prefix_path(app)
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "Default".to_string())
            });
        let args_label = self
            .cfg
            .games
            .get(app)
            .and_then(|g| g.launch_args.clone())
            .unwrap_or_else(|| "None".to_string());
        let install_path_label = self
            .cfg
            .games
            .get(app)
            .and_then(|g| g.install_path.clone())
            .or_else(|| {
                self.games
                    .iter()
                    .find(|g| g.app_name == app)
                    .and_then(|g| g.install_path.clone())
            })
            .unwrap_or_else(|| "Default".to_string());

        let mangohud = self
            .cfg
            .games
            .get(app)
            .and_then(|g| g.mangohud)
            .unwrap_or(self.cfg.default_mangohud);
        let gamemode = self
            .cfg
            .games
            .get(app)
            .and_then(|g| g.gamemode)
            .unwrap_or(self.cfg.default_gamemode);
        let lsfg = self
            .cfg
            .games
            .get(app)
            .and_then(|g| g.lsfg.clone())
            .unwrap_or_else(|| self.cfg.lsfg_multiplier.clone());
        let proton_str = self
            .cfg
            .games
            .get(app)
            .and_then(|g| g.proton.clone())
            .unwrap_or_else(|| format!("Default ({})", self.cfg.default_proton));

        let mut items = vec![
            ("Play".to_string(), Op::Play(app.to_string())),
            (
                format!(
                    "MangoHud:           {}",
                    if mangohud { "Yes" } else { "No" }
                ),
                Op::SetPerGameMangoHud(app.to_string()),
            ),
            (
                format!(
                    "GameMode:           {}",
                    if gamemode { "On" } else { "Off" }
                ),
                Op::SetPerGameGameMode(app.to_string()),
            ),
            (
                format!("LSFG Frame Gen:     {lsfg}"),
                Op::SetPerGameLsfg(app.to_string()),
            ),
            (
                format!("Proton Version:     {proton_str}"),
                Op::SetPerGameProton(app.to_string()),
            ),
            (
                format!("Launch Arguments:   {args_label}"),
                Op::SetPerGameLaunchArgs(app.to_string()),
            ),
            (
                format!("Wine/Proton Prefix: {pfx_label}"),
                Op::SetPerGamePrefix(app.to_string()),
            ),
            (
                format!("Move Game Location: {install_path_label}"),
                Op::MoveGame(app.to_string()),
            ),
            (
                "Update / Verify Installation".to_string(),
                Op::Update(app.to_string()),
            ),
            (
                "Delete game (keep prefix)".to_string(),
                Op::DeleteGame(app.to_string()),
            ),
            (
                "Delete game + prefix".to_string(),
                Op::DeleteGamePrefix(app.to_string()),
            ),
        ];
        items.push(if in_rofi {
            (
                "Remove Alt+G entry".to_string(),
                Op::RemoveEntry(app.to_string()),
            )
        } else {
            ("Add to Alt+G".to_string(), Op::AddEntry(app.to_string()))
        });
        items.push(("Game details".to_string(), Op::Details(app.to_string())));
        Menu {
            title: format!("{} (Launch Options)", self.title_of(app)),
            items,
            idx: 0,
            kind: MenuKind::Installed(app.to_string()),
        }
    }

    fn settings_menu(&self) -> Menu {
        let (sel_app, sel_title, is_installed) = self
            .current()
            .map(|g| (g.app_name.clone(), g.title.clone(), g.installed))
            .unwrap_or_default();
        Menu {
            title: "Global Settings".to_string(),
            items: vec![
                (
                    format!(
                        "Default install location: {}",
                        self.cfg.default_install_path
                    ),
                    Op::SetDefaultPath,
                ),
                (
                    format!("Dedicated prefix root:    {}", self.cfg.default_prefix_path),
                    Op::SetPrefixPath,
                ),
                (
                    format!("Default Proton:           {}", self.cfg.default_proton),
                    Op::SetDefaultProton,
                ),
                (
                    format!(
                        "Default MangoHud:         {}",
                        if self.cfg.default_mangohud {
                            "Yes"
                        } else {
                            "No"
                        }
                    ),
                    Op::SetDefaultMangoHud,
                ),
                (
                    format!(
                        "Default GameMode:         {}",
                        if self.cfg.default_gamemode {
                            "On"
                        } else {
                            "Off"
                        }
                    ),
                    Op::SetDefaultGameMode,
                ),
                (
                    format!("Default LSFG:             {}", self.cfg.lsfg_multiplier),
                    Op::SetLsfg,
                ),
                (
                    if is_installed {
                        format!("Move game location ({sel_title})")
                    } else if !sel_app.is_empty() {
                        format!("Set per-game install location ({sel_title})")
                    } else {
                        "Set per-game install location".to_string()
                    },
                    if is_installed {
                        Op::MoveGame(sel_app)
                    } else {
                        Op::SetPerGamePath(sel_app)
                    },
                ),
                ("Clean stale Alt+G entries".to_string(), Op::CleanStale),
            ],
            idx: 0,
            kind: MenuKind::Settings,
        }
    }

    fn menu_adjust(&mut self, dir: isize) {
        let op = match &self.mode {
            Mode::Menu(m) => match m.items.get(m.idx) {
                Some((_, o)) => o.clone(),
                None => return,
            },
            _ => return,
        };

        match op {
            Op::SetPerGameMangoHud(app) => {
                let cur = self
                    .cfg
                    .games
                    .get(&app)
                    .and_then(|g| g.mangohud)
                    .unwrap_or(self.cfg.default_mangohud);
                let next = !cur;
                self.cfg.games.entry(app.clone()).or_default().mangohud = Some(next);
                let _ = config::save(&self.cfg);
                self.say(format!(
                    "MangoHud for {}: {}",
                    self.title_of(&app),
                    if next { "Yes" } else { "No" }
                ));
                self.refresh_menu();
            }
            Op::SetDefaultMangoHud => {
                self.cfg.default_mangohud = !self.cfg.default_mangohud;
                let _ = config::save(&self.cfg);
                self.say(format!(
                    "Default MangoHud: {}",
                    if self.cfg.default_mangohud {
                        "Yes"
                    } else {
                        "No"
                    }
                ));
                self.refresh_menu();
            }
            Op::SetPerGameGameMode(app) => {
                let cur = self
                    .cfg
                    .games
                    .get(&app)
                    .and_then(|g| g.gamemode)
                    .unwrap_or(self.cfg.default_gamemode);
                let next = !cur;
                self.cfg.games.entry(app.clone()).or_default().gamemode = Some(next);
                let _ = config::save(&self.cfg);
                self.say(format!(
                    "GameMode for {}: {}",
                    self.title_of(&app),
                    if next { "On" } else { "Off" }
                ));
                self.refresh_menu();
            }
            Op::SetDefaultGameMode => {
                self.cfg.default_gamemode = !self.cfg.default_gamemode;
                let _ = config::save(&self.cfg);
                self.say(format!(
                    "Default GameMode: {}",
                    if self.cfg.default_gamemode {
                        "On"
                    } else {
                        "Off"
                    }
                ));
                self.refresh_menu();
            }
            Op::SetPerGameLsfg(app) => {
                let options = ["Disabled", "2x", "3x", "4x"];
                let cur = self
                    .cfg
                    .games
                    .get(&app)
                    .and_then(|g| g.lsfg.as_deref())
                    .unwrap_or(self.cfg.lsfg_multiplier.as_str());
                let cur_idx = options.iter().position(|&x| x == cur).unwrap_or(0);
                let next_idx = (cur_idx as isize + dir).rem_euclid(options.len() as isize) as usize;
                let next = options[next_idx];
                self.cfg.games.entry(app.clone()).or_default().lsfg = Some(next.to_string());
                let _ = config::save(&self.cfg);
                self.say(format!("LSFG for {}: {next}", self.title_of(&app)));
                self.refresh_menu();
            }
            Op::SetLsfg => {
                let options = ["Disabled", "2x", "3x", "4x"];
                let cur_idx = options
                    .iter()
                    .position(|&x| x == self.cfg.lsfg_multiplier.as_str())
                    .unwrap_or(0);
                let next_idx = (cur_idx as isize + dir).rem_euclid(options.len() as isize) as usize;
                let next = options[next_idx];
                self.cfg.lsfg_multiplier = next.to_string();
                let _ = config::save(&self.cfg);
                self.say(format!("Default LSFG: {next}"));
                self.refresh_menu();
            }
            Op::SetPerGameProton(app) => {
                let mut options = vec!["Default (Global)".to_string()];
                for p in proton::discover_protons() {
                    options.push(p.name);
                }
                let cur = self
                    .cfg
                    .games
                    .get(&app)
                    .and_then(|g| g.proton.clone())
                    .unwrap_or_else(|| "Default (Global)".into());
                let cur_idx = options.iter().position(|x| x == &cur).unwrap_or(0);
                let next_idx = (cur_idx as isize + dir).rem_euclid(options.len() as isize) as usize;
                let next = options[next_idx].clone();
                if next == "Default (Global)" {
                    self.cfg.games.entry(app.clone()).or_default().proton = None;
                    self.say(format!(
                        "Proton for {}: Default ({})",
                        self.title_of(&app),
                        self.cfg.default_proton
                    ));
                } else {
                    self.cfg.games.entry(app.clone()).or_default().proton = Some(next.clone());
                    self.say(format!("Proton for {}: {next}", self.title_of(&app)));
                }
                let _ = config::save(&self.cfg);
                self.refresh_menu();
            }
            Op::SetDefaultProton => {
                let mut options = vec!["Auto".to_string()];
                for p in proton::discover_protons() {
                    options.push(p.name);
                }
                let cur = &self.cfg.default_proton;
                let cur_idx = options.iter().position(|x| x == cur).unwrap_or(0);
                let next_idx = (cur_idx as isize + dir).rem_euclid(options.len() as isize) as usize;
                let next = options[next_idx].clone();
                self.cfg.default_proton = next.clone();
                let _ = config::save(&self.cfg);
                self.say(format!("Default Proton: {next}"));
                self.refresh_menu();
            }
            _ => {}
        }
    }

    fn refresh_menu(&mut self) {
        let kind = match &self.mode {
            Mode::Menu(m) => m.kind.clone(),
            _ => return,
        };
        let (new_title, new_items) = match kind {
            MenuKind::Installed(ref app) => {
                let fresh = self.installed_menu(app);
                (fresh.title, fresh.items)
            }
            MenuKind::Settings => {
                let fresh = self.settings_menu();
                (fresh.title, fresh.items)
            }
            _ => return,
        };
        if let Mode::Menu(ref mut m) = self.mode {
            m.title = new_title;
            m.items = new_items;
            if m.idx >= m.items.len() && !m.items.is_empty() {
                m.idx = m.items.len() - 1;
            }
        }
    }

    // ---- Input Handler ----------------------------------------------------
    pub fn handle(&mut self, intent: Intent) {
        self.dirty = true;

        if !matches!(intent, Intent::First) {
            self.pending_g = false;
        }

        // Active installation view: Esc or q cancels
        if let Mode::Install(_) = &self.mode {
            if matches!(intent, Intent::Cancel | Intent::Quit) {
                if let Some(mut inst) = self.active_install.take() {
                    inst.cancel();
                }
                self.mode = Mode::Library;
                self.say("Installation cancelled.");
                let _ = self.refresh_installed();
                return;
            }
        }

        // Text input dialogs consume chars and backspace
        if let Mode::Input { .. } = self.mode {
            self.handle_input(intent);
            return;
        }

        if self.searching {
            match intent {
                Intent::Char(c) => {
                    self.search.push(c);
                    self.apply_filter();
                    self.update_selected_details();
                    return;
                }
                Intent::Backspace => {
                    self.search.pop();
                    self.apply_filter();
                    self.update_selected_details();
                    return;
                }
                Intent::Cancel => {
                    self.searching = false;
                    self.search.clear();
                    self.apply_filter();
                    self.update_selected_details();
                    return;
                }
                Intent::Enter => {
                    self.searching = false;
                    return;
                }
                _ => {}
            }
        }

        if let Mode::Confirm { .. } = self.mode {
            match intent {
                Intent::ConfirmYes | Intent::Enter => {
                    self.confirm_yes();
                    return;
                }
                Intent::ConfirmNo | Intent::Cancel => {
                    self.mode = Mode::Library;
                    return;
                }
                _ => return,
            }
        }

        if let Mode::Menu(_) = self.mode {
            match intent {
                Intent::Up => {
                    if let Some(m) = self.menu_mut() {
                        m.idx = m.idx.saturating_sub(1);
                    }
                    return;
                }
                Intent::Down => {
                    if let Some(m) = self.menu_mut() {
                        let n = m.items.len();
                        if n > 0 {
                            m.idx = (m.idx + 1).min(n - 1);
                        }
                    }
                    return;
                }
                Intent::Left => {
                    self.menu_adjust(-1);
                    return;
                }
                Intent::Right => {
                    self.menu_adjust(1);
                    return;
                }
                Intent::Enter => {
                    self.menu_enter();
                    return;
                }
                Intent::Cancel => {
                    self.mode = Mode::Library;
                    return;
                }
                _ => {}
            }
        }

        match intent {
            Intent::Up => self.move_sel(-1),
            Intent::Down => self.move_sel(1),
            Intent::Left | Intent::Right => {}
            Intent::PageDown => self.move_sel(10),
            Intent::PageUp => self.move_sel(-10),
            Intent::First => {
                if self.pending_g {
                    self.pending_g = false;
                    self.jump_first();
                } else {
                    self.pending_g = true;
                    self.jump_first();
                }
            }
            Intent::Last => self.jump_last(),
            Intent::FilterCycle => self.cycle_filter(),
            Intent::ToggleSelect => self.toggle_select(),
            Intent::Search => {
                self.searching = true;
            }
            Intent::Refresh => {
                if self.refreshing {
                    self.say("Library refresh already in progress...");
                } else {
                    self.refreshing = true;
                    self.say("Refreshing library in background...");
                    let (tx, rx) = std::sync::mpsc::channel();
                    self.refresh_rx = Some(rx);
                    std::thread::spawn(move || {
                        let res = crate::legendary::library();
                        let _ = tx.send(res);
                    });
                }
            }
            Intent::Settings => {
                self.mode = Mode::Menu(self.settings_menu());
            }
            Intent::Help => self.mode = Mode::Help,
            Intent::Quit => {}
            Intent::Enter => self.on_enter(),
            Intent::Update => match self.installed_or_msg() {
                Some(a) => self.start_in_app_install(&a),
                None => self.say("Select an installed game first."),
            },
            Intent::DeleteMenu => self.quick_delete_menu(),
            Intent::Cancel => match self.mode {
                Mode::Library => {
                    if !self.selected_games.is_empty() {
                        let n = self.selected_games.len();
                        self.selected_games.clear();
                        self.say(format!("Deselected {n} games."));
                    }
                }
                _ => self.mode = Mode::Library,
            },
            Intent::ConfirmYes => self.confirm_yes(),
            Intent::ConfirmNo => {
                self.mode = Mode::Library;
            }
            Intent::DetailScrollDown => {
                self.detail_scroll = self.detail_scroll.saturating_add(2);
            }
            Intent::DetailScrollUp => {
                self.detail_scroll = self.detail_scroll.saturating_sub(2);
            }
            Intent::Char(_) | Intent::Backspace => {}
        }
    }

    fn move_sel(&mut self, delta: isize) {
        if self.filtered.is_empty() {
            return;
        }
        let total = self.filtered.len();
        let step = if delta >= 0 { 1 } else { -1 };
        let count = delta.abs();
        let mut curr = self.selected;

        for _ in 0..count {
            let mut next = curr;
            let mut found = false;
            for _ in 0..total {
                next = if step > 0 {
                    (next + 1) % total
                } else if next == 0 {
                    total - 1
                } else {
                    next - 1
                };
                if let RowItem::Game(_) = self.filtered[next] {
                    curr = next;
                    found = true;
                    break;
                }
            }
            if !found {
                break;
            }
        }
        self.selected = curr;
        self.detail_scroll = 0;
        self.update_selected_details();
    }

    fn on_enter(&mut self) {
        if !self.selected_games.is_empty() {
            let installed: Vec<String> = self
                .games
                .iter()
                .filter(|g| self.selected_games.contains(&g.app_name) && g.installed)
                .map(|g| g.app_name.clone())
                .collect();
            let uninstalled: Vec<String> = self
                .games
                .iter()
                .filter(|g| self.selected_games.contains(&g.app_name) && !g.installed)
                .map(|g| g.app_name.clone())
                .collect();

            if !installed.is_empty() {
                if installed.len() == 1 {
                    self.mode = Mode::Menu(self.installed_menu(&installed[0]));
                } else {
                    let n = installed.len();
                    self.mode = Mode::Menu(Menu {
                        title: format!("Manage {n} Selected Installed Games"),
                        items: vec![
                            (
                                format!("Delete {n} games (keep prefixes)"),
                                Op::DeleteBatch(installed.clone()),
                            ),
                            ("Clear selection".into(), Op::Back),
                        ],
                        idx: 0,
                        kind: MenuKind::BatchInstalled(installed),
                    });
                }
                return;
            }

            if !uninstalled.is_empty() {
                let n = uninstalled.len();
                let (base, _, _) = self.resolve_install(&uninstalled[0]);
                self.mode = Mode::Confirm {
                    lines: vec![
                        format!("Install {n} selected games?"),
                        "".into(),
                        format!("Target base directory: {base}"),
                        "".into(),
                        "Proceed with batch installation?".into(),
                    ],
                    op: Op::StartInstallBatch(uninstalled),
                };
                return;
            }
        }

        let Some(g) = self.current().cloned() else {
            return;
        };
        if g.installed {
            self.mode = Mode::Menu(self.installed_menu(&g.app_name));
        } else {
            let (base, folder, _) = self.resolve_install(&g.app_name);
            let target_dir = match &folder {
                Some(f) => format!("{base}/{f}"),
                None => format!("{base}/{}", g.title),
            };
            let details = self.current_details();
            let dl_str = details
                .and_then(|d| d.download_size)
                .map(prefix::fmt_size)
                .unwrap_or_else(|| "fetching...".into());
            let inst_str = details
                .and_then(|d| d.installed_size)
                .map(prefix::fmt_size)
                .unwrap_or_else(|| "fetching...".into());
            self.mode = Mode::Confirm {
                lines: vec![
                    format!("Install {}?", g.title),
                    "".into(),
                    format!("Target location: {target_dir}"),
                    format!("Download size:   {dl_str}"),
                    format!("Installed size:  {inst_str}"),
                    "".into(),
                    "Proceed with installation?".into(),
                ],
                op: Op::StartInstall(g.app_name),
            };
        }
    }

    fn menu_enter(&mut self) {
        let (label, op, kind) = match &self.mode {
            Mode::Menu(m) => match m.items.get(m.idx) {
                Some((l, o)) => (l.clone(), o.clone(), m.kind.clone()),
                None => return,
            },
            _ => return,
        };

        if matches!(kind, MenuKind::ProtonSelect { .. }) {
            // Picked a proton version from submenu
            let choice = label
                .split(" (")
                .next()
                .unwrap_or(&label)
                .trim()
                .to_string();
            match op {
                Op::SetDefaultProton => {
                    self.cfg.default_proton = choice.clone();
                    let _ = config::save(&self.cfg);
                    self.say(format!("Default Proton set to {choice}."));
                    self.mode = Mode::Library;
                    return;
                }
                Op::SetPerGameProton(app) => {
                    if choice.starts_with("Default") {
                        self.cfg.games.entry(app.clone()).or_default().proton = None;
                        self.say(format!(
                            "Proton override cleared for {}.",
                            self.title_of(&app)
                        ));
                    } else {
                        self.cfg.games.entry(app.clone()).or_default().proton =
                            Some(choice.clone());
                        self.say(format!(
                            "Proton for {} set to {choice}.",
                            self.title_of(&app)
                        ));
                    }
                    let _ = config::save(&self.cfg);
                    self.mode = Mode::Library;
                    return;
                }
                _ => {}
            }
        }

        match op {
            Op::SetPerGameMangoHud(_)
            | Op::SetDefaultMangoHud
            | Op::SetPerGameGameMode(_)
            | Op::SetDefaultGameMode
            | Op::SetPerGameLsfg(_)
            | Op::SetLsfg => {
                self.menu_adjust(1);
            }
            Op::Details(app) => {
                let _ = self.prefix_size(&app);
                self.mode = Mode::Details(app);
            }
            Op::SetDefaultPath => {
                let cur = self.cfg.default_install_path.clone();
                self.mode = Mode::Input {
                    title: "Default install location".into(),
                    buf: cur,
                    op: Op::SetDefaultPath,
                };
            }
            Op::SetPrefixPath => {
                let cur = self.cfg.default_prefix_path.clone();
                self.mode = Mode::Input {
                    title: "Dedicated prefix root directory".into(),
                    buf: cur,
                    op: Op::SetPrefixPath,
                };
            }
            Op::MoveGame(app) => {
                self.mode = Mode::Library;
                self.execute(Op::MoveGame(app));
            }
            Op::SetPerGamePath(app) => {
                if app.is_empty() {
                    self.say("Select a game first.");
                    self.mode = Mode::Library;
                    return;
                }
                let cur = self
                    .cfg
                    .games
                    .get(&app)
                    .and_then(|g| g.install_path.clone())
                    .unwrap_or_default();
                self.mode = Mode::Input {
                    title: "Install location override (empty = default)".into(),
                    buf: cur,
                    op: Op::SetPerGamePath(app),
                };
            }
            Op::SetPerGamePrefix(app) => {
                if app.is_empty() {
                    self.say("Select a game first.");
                    self.mode = Mode::Library;
                    return;
                }
                let cur = self
                    .cfg
                    .games
                    .get(&app)
                    .and_then(|g| g.prefix_path.clone())
                    .unwrap_or_default();
                self.mode = Mode::Input {
                    title: "Wine/Proton Prefix path (empty = default)".into(),
                    buf: cur,
                    op: Op::SetPerGamePrefix(app),
                };
            }
            Op::SetPerGameLaunchArgs(app) => {
                if app.is_empty() {
                    self.say("Select a game first.");
                    self.mode = Mode::Library;
                    return;
                }
                let cur = self
                    .cfg
                    .games
                    .get(&app)
                    .and_then(|g| g.launch_args.clone())
                    .unwrap_or_default();
                self.mode = Mode::Input {
                    title: "Launch Options / Arguments (empty = none)".into(),
                    buf: cur,
                    op: Op::SetPerGameLaunchArgs(app),
                };
            }
            Op::DeleteGame(app) => {
                let title = self.title_of(&app);
                self.mode = Mode::Confirm {
                    lines: vec![
                        "Delete game?".into(),
                        "".into(),
                        title,
                        "".into(),
                        "This removes:".into(),
                        "  Game files".into(),
                        "This keeps:".into(),
                        "  Dedicated Wine/Proton prefix".into(),
                        "  compatdata".into(),
                    ],
                    op: Op::DeleteGame(app),
                };
            }
            Op::DeleteGamePrefix(app) => {
                let title = self.title_of(&app);
                let pfx = self.prefix_path(&app);
                let (path_line, size_line) = match &pfx {
                    Some(p) => {
                        let n = self.prefix_size(&app).unwrap_or(0);
                        (p.display().to_string(), prefix::fmt_size(n))
                    }
                    None => ("<unknown — will NOT delete blindly>".into(), "—".into()),
                };
                if pfx.is_none() {
                    self.mode = Mode::Confirm {
                        lines: vec![
                            "Unable to safely identify the Wine/Proton prefix.".into(),
                            "".into(),
                            "Game files can still be removed.".into(),
                            "Prefix deletion was not performed.".into(),
                        ],
                        op: Op::Back,
                    };
                    return;
                }
                self.mode = Mode::Confirm {
                    lines: vec![
                        "Delete game + prefix?".into(),
                        "".into(),
                        title,
                        "".into(),
                        format!("Wine/Proton prefix: {path_line}"),
                        format!("Prefix size: {size_line}"),
                        "".into(),
                        "This will permanently remove:".into(),
                        "  Game files".into(),
                        "  Dedicated Wine/Proton prefix".into(),
                    ],
                    op: Op::DeleteGamePrefix(app),
                };
            }
            Op::RemoveEntry(app) => {
                let title = self.title_of(&app);
                self.mode = Mode::Confirm {
                    lines: vec![
                        "Remove Alt+G entry?".into(),
                        "".into(),
                        title,
                        "".into(),
                        "The game will remain installed.".into(),
                        "Only its Alt+G launcher entry will be removed.".into(),
                    ],
                    op: Op::RemoveEntry(app),
                };
            }
            Op::AddEntry(app) => self.execute(Op::AddEntry(app)),
            _ => {
                self.mode = Mode::Library;
                self.execute(op);
            }
        }
    }

    fn confirm_yes(&mut self) {
        let op = match &self.mode {
            Mode::Confirm { op, .. } => op.clone(),
            Mode::Menu(_) => {
                self.menu_enter();
                return;
            }
            _ => return,
        };
        self.mode = Mode::Library;
        self.execute(op);
    }

    fn handle_input(&mut self, intent: Intent) {
        let (title, buf, op) = match &mut self.mode {
            Mode::Input { title, buf, op } => (title.clone(), buf, op.clone()),
            _ => return,
        };
        match intent {
            Intent::Char(c) => buf.push(c),
            Intent::Backspace => {
                buf.pop();
            }
            Intent::Enter => {
                let value = buf.trim().to_string();
                match op {
                    Op::SetDefaultPath => {
                        if value.is_empty() {
                            self.say("Empty path ignored.");
                        } else if let Err(e) = config::validate_dir(&value) {
                            self.say(format!("invalid path: {e}"));
                            return;
                        } else {
                            self.cfg.default_install_path = value;
                            let _ = config::save(&self.cfg);
                            self.say("Default install location saved.");
                        }
                        self.mode = Mode::Library;
                    }
                    Op::SetPrefixPath => {
                        if value.is_empty() {
                            self.say("Empty prefix path ignored.");
                        } else {
                            self.cfg.default_prefix_path = value;
                            let _ = config::save(&self.cfg);
                            self.say("Dedicated prefix root saved.");
                        }
                        self.mode = Mode::Library;
                    }
                    Op::MoveGame(app) => {
                        let dest = value.trim().to_string();
                        if dest.is_empty() {
                            self.say("Move destination cannot be empty.");
                            self.mode = Mode::Library;
                        } else {
                            let title = self.title_of(&app);
                            let cur_path = self
                                .games
                                .iter()
                                .find(|g| g.app_name == app)
                                .and_then(|g| g.install_path.clone())
                                .unwrap_or_else(|| "Unknown".into());
                            self.mode = Mode::Confirm {
                                lines: vec![
                                    format!("Move {title}?"),
                                    format!("Current path:     {cur_path}"),
                                    format!("Destination base: {dest}"),
                                    "".into(),
                                    "Legendary will move game files and update database.".into(),
                                ],
                                op: Op::DoMoveGame { app, dest },
                            };
                        }
                    }
                    Op::SetPerGamePath(app) => {
                        if value.is_empty() {
                            self.cfg.games.remove(&app);
                            let _ = config::save(&self.cfg);
                            self.say("Per-game override cleared.");
                        } else if let Err(e) = config::validate_dir(&value) {
                            self.say(format!("invalid path: {e}"));
                            return;
                        } else {
                            self.cfg.games.entry(app).or_default().install_path = Some(value);
                            let _ = config::save(&self.cfg);
                            self.say("Per-game location saved.");
                        }
                        self.mode = Mode::Library;
                    }
                    Op::SetPerGamePrefix(app) => {
                        if value.is_empty() {
                            self.cfg.games.entry(app).or_default().prefix_path = None;
                            let _ = config::save(&self.cfg);
                            self.say("Custom prefix cleared (using default).");
                        } else {
                            self.cfg.games.entry(app).or_default().prefix_path = Some(value);
                            let _ = config::save(&self.cfg);
                            self.say("Per-game prefix saved.");
                        }
                        self.mode = Mode::Library;
                    }
                    Op::SetPerGameLaunchArgs(app) => {
                        if value.is_empty() {
                            self.cfg.games.entry(app).or_default().launch_args = None;
                            let _ = config::save(&self.cfg);
                            self.say("Launch arguments cleared.");
                        } else {
                            self.cfg.games.entry(app).or_default().launch_args = Some(value);
                            let _ = config::save(&self.cfg);
                            self.say("Launch arguments saved.");
                        }
                        self.mode = Mode::Library;
                    }
                    _ => self.mode = Mode::Library,
                }
                return;
            }
            Intent::Cancel => {
                self.mode = Mode::Library;
            }
            _ => {}
        }
        let _ = title;
    }

    fn installed_or_msg(&self) -> Option<String> {
        match self.current() {
            Some(g) if g.installed => Some(g.app_name.clone()),
            _ => None,
        }
    }

    fn quick_delete_menu(&mut self) {
        if !self.selected_games.is_empty() {
            let installed: Vec<String> = self
                .games
                .iter()
                .filter(|g| self.selected_games.contains(&g.app_name) && g.installed)
                .map(|g| g.app_name.clone())
                .collect();
            if installed.is_empty() {
                self.say("None of the selected games are installed.");
                return;
            }
            let n = installed.len();
            let app_list = installed.clone();
            self.mode = Mode::Confirm {
                lines: vec![
                    format!("Delete {n} selected games?"),
                    "".into(),
                    "Game files will be uninstalled via Legendary.".into(),
                    "Wine/Proton prefixes will be preserved.".into(),
                ],
                op: Op::DeleteBatch(app_list),
            };
            return;
        }

        if let Some(g) = self.current().cloned() {
            if !g.installed {
                self.say("Not installed — nothing to delete.");
                return;
            }
            let app = g.app_name.clone();
            self.mode = Mode::Menu(Menu {
                title: self.title_of(&app),
                items: vec![
                    (
                        "Delete game (keep prefix)".into(),
                        Op::DeleteGame(app.clone()),
                    ),
                    (
                        "Delete game + prefix".into(),
                        Op::DeleteGamePrefix(app.clone()),
                    ),
                    ("Remove Alt+G entry".into(), Op::RemoveEntry(app.clone())),
                ],
                idx: 0,
                kind: MenuKind::DeleteConfirm(app),
            });
        }
    }

    // ---- View Helpers for ui.rs -------------------------------------------
    pub fn mode(&self) -> &Mode {
        &self.mode
    }

    pub fn menu_mut(&mut self) -> Option<&mut Menu> {
        match &mut self.mode {
            Mode::Menu(m) => Some(m),
            _ => None,
        }
    }

    pub fn filtered_rows(&self) -> &[RowItem] {
        &self.filtered
    }

    pub fn detail_for(&self, app: &str) -> Vec<(String, String)> {
        let g = match self.games.iter().find(|g| g.app_name == app) {
            Some(g) => g,
            None => return vec![],
        };
        let mut rows = vec![
            ("Title".into(), g.title.clone()),
            ("App ID".into(), g.app_name.clone()),
            (
                "Installed".into(),
                if g.installed {
                    "Yes".into()
                } else {
                    "No".into()
                },
            ),
        ];
        if let Some(v) = &g.version {
            rows.push(("Version".into(), v.clone()));
        }
        if let Some(p) = &g.install_path {
            rows.push(("Location".into(), p.clone()));
        }
        let pp = self.prefix_path(app);
        let pp_str = pp.as_ref().map(|p| p.display().to_string());
        rows.push((
            "Prefix".into(),
            pp_str.unwrap_or_else(|| format!("{}/{}", self.cfg.default_prefix_path, app)),
        ));
        rows.push((
            "Proton".into(),
            self.cfg
                .games
                .get(app)
                .and_then(|g| g.proton.clone())
                .unwrap_or_else(|| format!("Default ({})", self.cfg.default_proton)),
        ));
        rows.push((
            "GameMode".into(),
            if self
                .cfg
                .games
                .get(app)
                .and_then(|g| g.gamemode)
                .unwrap_or(self.cfg.default_gamemode)
            {
                "Enabled".into()
            } else {
                "Disabled".into()
            },
        ));
        rows.push((
            "Alt+G entry".into(),
            if self.rofi_entries.iter().any(|e| e.appid == app) {
                "Present".into()
            } else {
                "Missing".into()
            },
        ));
        if let Some(d) = self.metadata_mgr.load_from_cache(app) {
            if let Some(dl) = d.download_size {
                rows.push(("Download Size".into(), prefix::fmt_size(dl)));
            }
            if let Some(inst) = d.installed_size {
                rows.push(("Installed Size".into(), prefix::fmt_size(inst)));
            }
            if let Some(exe) = &d.launch_exe {
                rows.push(("Executable".into(), exe.clone()));
            }
            if let Some(bid) = &d.build_id {
                rows.push(("Build ID".into(), bid.clone()));
            }
            if let Some(plt) = &d.platform {
                rows.push(("Platform".into(), plt.clone()));
            }
            if let Some(cs) = d.cloud_saves {
                let cs_str = if cs {
                    match &d.cloud_save_folder {
                        Some(f) => format!("Supported ({f})"),
                        None => "Supported".into(),
                    }
                } else {
                    "Not supported".into()
                };
                rows.push(("Cloud Saves".into(), cs_str));
            }
            if !d.prerequisites.is_empty() {
                rows.push(("Prerequisites".into(), d.prerequisites.join(", ")));
            }
            if !d.installed_dlc.is_empty() {
                rows.push(("Installed DLC".into(), d.installed_dlc.join(", ")));
            }
            if !d.owned_dlc.is_empty() {
                rows.push(("Owned DLC".into(), format!("{} DLCs", d.owned_dlc.len())));
            }
            if let Some(dev) = &d.developer {
                rows.push(("Developer".into(), dev.clone()));
            }
            if let Some(publ) = &d.publisher {
                rows.push(("Publisher".into(), publ.clone()));
            }
            if let Some(date) = d.release_date.as_ref().or(d.grant_date.as_ref()) {
                rows.push(("Date".into(), date.clone()));
            }
        }
        if let Some(args) = self.cfg.games.get(app).and_then(|g| g.launch_args.clone()) {
            rows.push(("Launch Arguments".into(), args));
        }
        rows
    }
}

pub(crate) use Mode as AppMode;

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app() -> App {
        let mut app = App {
            games: vec![
                Game {
                    app_name: "app1".into(),
                    title: "Game Alpha".into(),
                    installed: false,
                    version: None,
                    install_path: None,
                },
                Game {
                    app_name: "app2".into(),
                    title: "Game Beta".into(),
                    installed: true,
                    version: Some("1.0".into()),
                    install_path: Some("/mnt/games/beta".into()),
                },
                Game {
                    app_name: "app3".into(),
                    title: "Game Gamma".into(),
                    installed: false,
                    version: None,
                    install_path: None,
                },
            ],
            filtered: Vec::new(),
            selected: 0,
            detail_scroll: 0,
            search: String::new(),
            searching: false,
            mode: Mode::Library,
            status: String::new(),
            cfg: config::Config::default(),
            prefix_sizes: HashMap::new(),
            rofi_entries: Vec::new(),
            dirty: false,
            suspended: false,
            filter: Filter::All,
            pending_g: false,
            theme: crate::theme::load(),
            metadata_mgr: metadata::MetadataManager::new(),
            active_install: None,
            selected_details: None,
            selected_games: HashSet::new(),
            install_queue: Vec::new(),
            refreshing: false,
            refresh_rx: None,
        };
        app.apply_filter();
        app.update_selected_details();
        app
    }

    #[test]
    fn ensure_valid_selection_out_of_bounds_resilience() {
        let mut app = test_app();
        // With 5 items total, calling ensure_valid_selection with 99 must not panic
        app.ensure_valid_selection(99);
        assert!(app.selected < app.filtered.len());

        // Even with an empty list, ensure_valid_selection must not panic
        app.filtered.clear();
        app.ensure_valid_selection(99);
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn navigation_and_bounds() {
        let mut app = test_app();
        // Index 0: Header("Installed Games", 1)
        // Index 1: Game(Beta)
        // Index 2: Header("Library", 2)
        // Index 3: Game(Alpha)
        // Index 4: Game(Gamma)
        assert_eq!(app.selected, 1);
        assert_eq!(app.current().unwrap().app_name, "app2");

        app.handle(Intent::Down); // skips header at index 2, lands on index 3
        assert_eq!(app.selected, 3);
        assert_eq!(app.current().unwrap().app_name, "app1");

        app.handle(Intent::Down); // lands on index 4
        assert_eq!(app.selected, 4);
        assert_eq!(app.current().unwrap().app_name, "app3");

        app.handle(Intent::Down); // wrap around, skips header 0, lands on 1
        assert_eq!(app.selected, 1);
        assert_eq!(app.current().unwrap().app_name, "app2");

        app.handle(Intent::Up); // wrap around to index 4
        assert_eq!(app.selected, 4);
        assert_eq!(app.current().unwrap().app_name, "app3");

        app.handle(Intent::First);
        assert_eq!(app.selected, 1);
        assert_eq!(app.current().unwrap().app_name, "app2");

        app.handle(Intent::Last);
        assert_eq!(app.selected, 4);
        assert_eq!(app.current().unwrap().app_name, "app3");
    }

    #[test]
    fn deterministic_gg() {
        let mut app = test_app();
        app.selected = 4;

        // First 'g' arms pending_g
        app.handle(Intent::First);
        assert!(app.pending_g);
        assert_eq!(app.selected, 1);

        // Another key resets pending_g
        app.selected = 4;
        app.handle(Intent::Down);
        assert!(!app.pending_g);

        // Double 'g' (gg)
        app.handle(Intent::First);
        assert!(app.pending_g);
        app.handle(Intent::First);
        assert!(!app.pending_g);
        assert_eq!(app.selected, 1);
    }

    #[test]
    fn filter_cycling() {
        let mut app = test_app();
        assert_eq!(app.filter, Filter::All);
        assert_eq!(app.filtered.len(), 5);

        app.handle(Intent::FilterCycle);
        assert_eq!(app.filter, Filter::Installed);
        assert_eq!(app.filtered.len(), 2);
        assert_eq!(app.current().unwrap().app_name, "app2");

        app.handle(Intent::FilterCycle);
        assert_eq!(app.filter, Filter::Available);
        assert_eq!(app.filtered.len(), 3);
        assert_eq!(app.current().unwrap().app_name, "app1");

        app.handle(Intent::FilterCycle);
        assert_eq!(app.filter, Filter::All);
        assert_eq!(app.filtered.len(), 5);
    }

    #[test]
    fn toggle_select_flow() {
        let mut app = test_app();
        assert!(app.selected_games.is_empty());
        app.selected = 3; // Alpha (uninstalled)
        assert_eq!(app.current().unwrap().app_name, "app1");

        // Pressing Tab toggles selection on game 3 (Alpha) and advances to 4 (Gamma)
        app.handle(Intent::ToggleSelect);
        assert_eq!(app.selected_games.len(), 1);
        assert!(app.selected_games.contains("app1"));
        assert_eq!(app.selected, 4);
        assert_eq!(app.current().unwrap().app_name, "app3");

        // Move to Beta (installed, index 1)
        app.selected = 1;
        // Pressing Tab on Beta (installed) while Alpha (uninstalled) is selected is blocked by guardrail!
        app.handle(Intent::ToggleSelect);
        assert_eq!(app.selected_games.len(), 1);
        assert!(app.status.contains("Cannot mix"));

        // Move to game 4 (Gamma, uninstalled) and press Tab -> succeeds!
        app.selected = 4;
        app.handle(Intent::ToggleSelect);
        assert_eq!(app.selected_games.len(), 2);

        // Pressing Enter on uninstalled games prompts confirmation, then confirming starts install
        app.handle(Intent::Enter);
        assert!(matches!(app.mode, Mode::Confirm { .. }));
        app.handle(Intent::ConfirmYes);
        assert!(matches!(app.mode, Mode::Install(_)));

        // Reset and test installed guardrail
        let mut app2 = test_app();
        app2.selected = 1; // Beta (installed)
        app2.handle(Intent::ToggleSelect);
        assert_eq!(app2.selected_games.len(), 1);

        // Attempting to select Alpha (uninstalled) while Beta (installed) is selected is blocked
        app2.selected = 3;
        app2.handle(Intent::ToggleSelect);
        assert_eq!(app2.selected_games.len(), 1);
        assert!(app2.status.contains("Cannot mix"));

        // Pressing Enter on installed selection opens management menu
        app2.handle(Intent::Enter);
        assert!(matches!(app2.mode, Mode::Menu(_)));

        // Cancel (Esc) clears selections
        app2.handle(Intent::Cancel);
        assert!(matches!(app2.mode, Mode::Library));
    }

    #[test]
    fn detail_scrolling() {
        let mut app = test_app();
        assert_eq!(app.detail_scroll, 0);

        app.handle(Intent::DetailScrollDown);
        assert_eq!(app.detail_scroll, 2);

        app.handle(Intent::DetailScrollDown);
        assert_eq!(app.detail_scroll, 4);

        app.handle(Intent::DetailScrollUp);
        assert_eq!(app.detail_scroll, 2);

        // Moving to another game resets detail_scroll
        app.handle(Intent::Down);
        assert_eq!(app.detail_scroll, 0);
    }

    #[test]
    fn menu_adjust_flow() {
        let mut app = test_app();
        app.selected = 1; // Beta (installed)
        app.handle(Intent::Enter);
        assert!(matches!(app.mode, Mode::Menu(_)));

        // Index 1 is MangoHud
        if let Mode::Menu(ref mut m) = app.mode {
            m.idx = 1;
        }
        assert_eq!(app.cfg.games.get("app2").and_then(|g| g.mangohud), None);
        app.handle(Intent::Right);
        assert_eq!(
            app.cfg.games.get("app2").and_then(|g| g.mangohud),
            Some(false)
        );
        app.handle(Intent::Left);
        assert_eq!(
            app.cfg.games.get("app2").and_then(|g| g.mangohud),
            Some(true)
        );

        // Index 2 is GameMode
        if let Mode::Menu(ref mut m) = app.mode {
            m.idx = 2;
        }
        app.handle(Intent::Right);
        assert_eq!(
            app.cfg.games.get("app2").and_then(|g| g.gamemode),
            Some(false)
        );
        app.handle(Intent::Left);
        assert_eq!(
            app.cfg.games.get("app2").and_then(|g| g.gamemode),
            Some(true)
        );

        // Index 3 is LSFG: Disabled -> 2x -> 3x -> 4x -> Disabled
        if let Mode::Menu(ref mut m) = app.mode {
            m.idx = 3;
        }
        app.handle(Intent::Right);
        assert_eq!(
            app.cfg.games.get("app2").and_then(|g| g.lsfg.as_deref()),
            Some("2x")
        );
        app.handle(Intent::Right);
        assert_eq!(
            app.cfg.games.get("app2").and_then(|g| g.lsfg.as_deref()),
            Some("3x")
        );
        app.handle(Intent::Right);
        assert_eq!(
            app.cfg.games.get("app2").and_then(|g| g.lsfg.as_deref()),
            Some("4x")
        );
        app.handle(Intent::Right);
        assert_eq!(
            app.cfg.games.get("app2").and_then(|g| g.lsfg.as_deref()),
            Some("Disabled")
        );
        app.handle(Intent::Left);
        assert_eq!(
            app.cfg.games.get("app2").and_then(|g| g.lsfg.as_deref()),
            Some("4x")
        );
    }

    #[test]
    fn move_game_menu_option() {
        let mut app = test_app();
        app.selected = 1; // Beta (installed)

        // Settings menu shows "Move game location (Game Beta)"
        let settings = app.settings_menu();
        assert!(settings.items.iter().any(|(lbl, op)| {
            lbl.contains("Move game location (Game Beta)")
                && matches!(op, Op::MoveGame(id) if id == "app2")
        }));

        // Installed menu shows "Move Game Location: ..."
        let inst_menu = app.installed_menu("app2");
        assert!(inst_menu.items.iter().any(|(lbl, op)| {
            lbl.contains("Move Game Location:") && matches!(op, Op::MoveGame(id) if id == "app2")
        }));

        // When uninstalled game is selected (Alpha, index 3), settings menu shows "Set per-game install location"
        app.selected = 3;
        let settings_uninstalled = app.settings_menu();
        assert!(settings_uninstalled.items.iter().any(|(lbl, op)| {
            lbl.contains("Set per-game install location (Game Alpha)")
                && matches!(op, Op::SetPerGamePath(id) if id == "app1")
        }));
    }
}

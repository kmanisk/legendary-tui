//! All rendering for egs. Read-only over App; no side effects, no animation.
//! Follows the desktop theme: accent fills, solid backgrounds, and high-contrast text.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame,
};

use crate::app::{App, AppMode};
use crate::prefix;

fn centered(r: Rect, w: u16, h: u16) -> Rect {
    let x = r.x.saturating_add(r.width.saturating_sub(w) / 2);
    let y = r.y.saturating_add(r.height.saturating_sub(h) / 2);
    Rect {
        x,
        y,
        width: w.min(r.width),
        height: h.min(r.height),
    }
}

fn title_style(app: &App) -> Style {
    Style::default()
        .fg(app.theme.accent)
        .add_modifier(Modifier::BOLD)
}

fn solid_block<'a>(title: &'a str, theme: &crate::theme::Theme) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent))
        .style(Style::default().bg(theme.bg2).fg(theme.fg))
        .title(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
}

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(3),
        ])
        .split(area);

    // Header
    let count_text = if app.selected_games.is_empty() {
        format!("  {} games", app.games.len())
    } else {
        format!(
            "  {} games  ({} selected)",
            app.games.len(),
            app.selected_games.len()
        )
    };
    let mut header_spans = vec![
        Span::styled(" Epic Games ", title_style(app)),
        Span::styled(count_text, Style::default().fg(app.theme.fg)),
    ];
    if let Some(inst) = &app.active_install {
        let q_str = if app.install_queue.is_empty() {
            String::new()
        } else {
            format!(" (+{} queued)", app.install_queue.len())
        };
        header_spans.push(Span::styled(
            format!(
                "   [↓ {} {:.1}% • {} • ETA {}]{}",
                inst.title,
                inst.progress.percentage,
                inst.progress.speed_str,
                inst.progress.eta_str,
                q_str
            ),
            Style::default()
                .fg(app.theme.cyan)
                .add_modifier(Modifier::BOLD),
        ));
    } else if !app.install_queue.is_empty() {
        header_spans.push(Span::styled(
            format!("   [{} games queued for download]", app.install_queue.len()),
            Style::default()
                .fg(app.theme.yellow)
                .add_modifier(Modifier::BOLD),
        ));
    }
    if !app.status.is_empty() {
        header_spans.push(Span::styled(
            format!("      {}", app.status),
            Style::default().fg(app.theme.yellow),
        ));
    }
    let header = Paragraph::new(vec![Line::from(header_spans)]).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(app.theme.muted))
            .style(Style::default().bg(app.theme.bg).fg(app.theme.fg)),
    );
    f.render_widget(header, rows[0]);

    // Body: list | detail
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(rows[1]);
    draw_list(f, app, cols[0]);
    draw_detail(f, app, cols[1]);

    // Footer
    let footer_text = if app.searching {
        vec![
            Line::from(vec![
                Span::styled(
                    "Search: ",
                    Style::default()
                        .fg(app.theme.accent)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(&app.search, Style::default().fg(app.theme.fg)),
                Span::styled(
                    "  [Enter] done · [Esc] clear",
                    Style::default().fg(app.theme.muted),
                ),
            ]),
            Line::from(Span::styled(
                app.status_line(),
                Style::default().fg(app.theme.cyan),
            )),
        ]
    } else {
        let is_installed = if !app.selected_games.is_empty() {
            app.selected_games.iter().all(|id| {
                app.games
                    .iter()
                    .find(|gm| &gm.app_name == id)
                    .map(|gm| gm.installed)
                    .unwrap_or(false)
            })
        } else {
            app.current().map(|g| g.installed).unwrap_or(false)
        };

        let key_style = Style::default()
            .fg(app.theme.accent)
            .add_modifier(Modifier::BOLD);
        let fg_style = Style::default().fg(app.theme.fg);

        let mut spans = vec![
            Span::styled("j/k", key_style),
            Span::styled(" Navigate  ", fg_style),
        ];

        if is_installed {
            spans.push(Span::styled("Enter", key_style));
            spans.push(Span::styled(" Launch Options  ", fg_style));
            spans.push(Span::styled("d", key_style));
            spans.push(Span::styled(" Delete  ", fg_style));
        } else {
            spans.push(Span::styled("Enter", key_style));
            spans.push(Span::styled(" Install  ", fg_style));
        }

        if app.active_install.is_some() || !app.install_queue.is_empty() {
            spans.push(Span::styled("c", key_style));
            spans.push(Span::styled(" Cancel/Dequeue  ", fg_style));
        }

        spans.extend(vec![
            Span::styled("Tab", key_style),
            Span::styled(" Select  ", fg_style),
            Span::styled("f", key_style),
            Span::styled(" Filter  ", fg_style),
            Span::styled("r", key_style),
            Span::styled(" Refresh  ", fg_style),
            Span::styled("s", key_style),
            Span::styled(" Settings  ", fg_style),
            Span::styled("?", key_style),
            Span::styled(" Help  ", fg_style),
            Span::styled("q", key_style),
            Span::styled(" Quit", fg_style),
        ]);

        vec![
            Line::from(spans),
            Line::from(Span::styled(
                app.status_line(),
                Style::default().fg(app.theme.cyan),
            )),
        ]
    };

    let footer = Paragraph::new(footer_text).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(app.theme.muted))
            .style(Style::default().bg(app.theme.bg).fg(app.theme.fg)),
    );
    f.render_widget(footer, rows[2]);

    match app.mode() {
        AppMode::Menu(_) => draw_menu_popup(f, app, area),
        AppMode::Confirm { .. } => draw_confirm_popup(f, app, area),
        AppMode::Input { .. } => draw_input_popup(f, app, area),
        AppMode::Details(_) => draw_details_popup(f, app, area),
        AppMode::Install(prog) => draw_install_popup(f, app, prog, area),
        AppMode::Help => draw_help_popup(f, app, area),
        AppMode::Library => {}
    }
}

fn draw_list(f: &mut Frame, app: &App, area: Rect) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(area);

    let search_title = format!("Search [{}]", app.filter.label());
    let search = Paragraph::new(format!("> {}", app.search)).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(if app.searching {
                app.theme.accent
            } else {
                app.theme.muted
            }))
            .title(Span::styled(
                format!(" {search_title} "),
                Style::default().fg(app.theme.accent),
            ))
            .style(Style::default().bg(app.theme.bg).fg(app.theme.fg)),
    );
    f.render_widget(search, rows[0]);

    let items: Vec<ListItem> = app
        .filtered_rows()
        .iter()
        .enumerate()
        .map(|(i, row)| match row {
            crate::app::RowItem::Header(title, count) => {
                let banner = format!("── {title} ({count}) ──");
                let line = Line::from(vec![Span::styled(
                    banner,
                    Style::default()
                        .fg(app.theme.accent)
                        .add_modifier(Modifier::BOLD),
                )]);
                ListItem::new(line)
            }
            crate::app::RowItem::Game(idx) => {
                let g = &app.games[*idx];
                let is_sel = i == app.selected;
                let is_tagged = app.selected_games.contains(&g.app_name);

                let check_str = if is_tagged { "[✓] " } else { "[ ] " };
                let check_style = if is_sel {
                    Style::default()
                        .fg(app.theme.on_accent)
                        .add_modifier(Modifier::BOLD)
                } else if is_tagged {
                    Style::default()
                        .fg(app.theme.accent)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(app.theme.muted)
                };

                let inst_str = if g.installed { "● " } else { "  " };
                let inst_style = if is_sel {
                    Style::default()
                        .fg(app.theme.on_accent)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                        .fg(app.theme.green)
                        .add_modifier(Modifier::BOLD)
                };

                let title_style = if is_sel {
                    Style::default()
                        .fg(app.theme.on_accent)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(app.theme.fg)
                };

                let mut spans = vec![
                    Span::styled(check_str, check_style),
                    Span::styled(inst_str, inst_style),
                    Span::styled(g.title.clone(), title_style),
                ];
                let is_active_dl =
                    app.active_install.as_ref().map(|i| &i.app_name) == Some(&g.app_name);
                let queue_pos = app.install_queue.iter().position(|q| q == &g.app_name);

                if is_active_dl {
                    let perc = app
                        .active_install
                        .as_ref()
                        .map(|i| i.progress.percentage)
                        .unwrap_or(0.0);
                    spans.push(Span::styled(
                        format!(" [↓ {:.0}%]", perc),
                        if is_sel {
                            Style::default()
                                .fg(app.theme.on_accent)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default()
                                .fg(app.theme.cyan)
                                .add_modifier(Modifier::BOLD)
                        },
                    ));
                } else if let Some(pos) = queue_pos {
                    spans.push(Span::styled(
                        format!(" [QUEUED #{}]", pos + 1),
                        if is_sel {
                            Style::default()
                                .fg(app.theme.on_accent)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default()
                                .fg(app.theme.yellow)
                                .add_modifier(Modifier::BOLD)
                        },
                    ));
                } else if g.needs_update {
                    spans.push(Span::styled(
                        " [UPDATE]",
                        if is_sel {
                            Style::default()
                                .fg(app.theme.on_accent)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default()
                                .fg(app.theme.yellow)
                                .add_modifier(Modifier::BOLD)
                        },
                    ));
                }
                let line = Line::from(spans);
                ListItem::new(line)
            }
        })
        .collect();

    let mut state = ratatui::widgets::ListState::default();
    state.select(Some(app.selected));
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.theme.sel))
                .style(Style::default().bg(app.theme.bg).fg(app.theme.fg))
                .title(Span::styled(
                    " Library ",
                    Style::default().fg(app.theme.accent),
                )),
        )
        .highlight_symbol("> ")
        .highlight_style(
            Style::default()
                .bg(app.theme.accent)
                .fg(app.theme.on_accent)
                .add_modifier(Modifier::BOLD),
        );
    f.render_stateful_widget(list, rows[1], &mut state);
}

fn protondb_badge<'a>(tier: Option<&str>, theme: &'a crate::theme::Theme) -> (Span<'a>, Span<'a>) {
    let label = Span::styled("ProtonDB:     ", Style::default().fg(theme.cyan));
    let val = match tier {
        Some(t) => {
            let style = match t.to_lowercase().as_str() {
                "platinum" => Style::default().fg(theme.cyan).add_modifier(Modifier::BOLD),
                "gold" => Style::default()
                    .fg(theme.yellow)
                    .add_modifier(Modifier::BOLD),
                "silver" => Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
                "bronze" => Style::default()
                    .fg(Color::Rgb(205, 127, 50))
                    .add_modifier(Modifier::BOLD),
                "borked" => Style::default().fg(theme.red).add_modifier(Modifier::BOLD),
                "native" => Style::default()
                    .fg(theme.green)
                    .add_modifier(Modifier::BOLD),
                _ => Style::default().fg(theme.fg),
            };
            Span::styled(t.to_string(), style)
        }
        None => Span::styled("Checking...", Style::default().fg(theme.muted)),
    };
    (label, val)
}

fn draw_detail(f: &mut Frame, app: &App, area: Rect) {
    let mut lines = Vec::new();
    match app.current() {
        Some(g) => {
            let details = app.current_details();

            // Title
            lines.push(Line::from(vec![Span::styled(
                &g.title,
                Style::default()
                    .fg(app.theme.accent)
                    .add_modifier(Modifier::BOLD),
            )]));
            lines.push(Line::from(""));

            if g.installed {
                // State: Installed
                let mut state_spans = vec![
                    Span::styled("State:        ", Style::default().fg(app.theme.cyan)),
                    Span::styled(
                        "Installed",
                        Style::default()
                            .fg(app.theme.green)
                            .add_modifier(Modifier::BOLD),
                    ),
                ];
                if g.needs_update {
                    let avail = g.available_version.as_deref().unwrap_or("latest");
                    state_spans.push(Span::styled(
                        format!(" [UPDATE: -> {avail}]"),
                        Style::default()
                            .fg(app.theme.yellow)
                            .add_modifier(Modifier::BOLD),
                    ));
                }
                if let Some(inst) = &app.active_install {
                    if inst.app_name == g.app_name {
                        state_spans.push(Span::styled(
                            format!(" [UPDATING: {:.1}%]", inst.progress.percentage),
                            Style::default()
                                .fg(app.theme.cyan)
                                .add_modifier(Modifier::BOLD),
                        ));
                    }
                }
                lines.push(Line::from(state_spans));

                // Clean Version (without noisy hash)
                let ver = g
                    .version
                    .as_deref()
                    .or_else(|| details.and_then(|d| d.version.as_deref()))
                    .unwrap_or("N/A");
                lines.push(Line::from(vec![
                    Span::styled("Version:      ", Style::default().fg(app.theme.cyan)),
                    Span::styled(ver, Style::default().fg(app.theme.fg)),
                ]));

                // ProtonDB Rating
                let pdb_tier = details
                    .and_then(|d| d.protondb_tier.as_deref())
                    .or_else(|| app.metadata_mgr.get_protondb_tier(&g.app_name));
                let (pdb_lbl, pdb_val) = protondb_badge(pdb_tier, &app.theme);
                lines.push(Line::from(vec![pdb_lbl, pdb_val]));

                // Installed Size
                if let Some(inst) = details.and_then(|d| d.installed_size) {
                    lines.push(Line::from(vec![
                        Span::styled("Installed:    ", Style::default().fg(app.theme.cyan)),
                        Span::styled(prefix::fmt_size(inst), Style::default().fg(app.theme.fg)),
                    ]));
                }

                // Executable
                if let Some(exe) = details.and_then(|d| d.launch_exe.as_deref()) {
                    lines.push(Line::from(vec![
                        Span::styled("Executable:   ", Style::default().fg(app.theme.cyan)),
                        Span::styled(exe, Style::default().fg(app.theme.fg)),
                    ]));
                }

                // Proton
                let proton = app
                    .cfg
                    .games
                    .get(&g.app_name)
                    .and_then(|c| c.proton.clone())
                    .unwrap_or_else(|| format!("Default ({})", app.cfg.default_proton));
                lines.push(Line::from(vec![
                    Span::styled("Proton:       ", Style::default().fg(app.theme.cyan)),
                    Span::styled(proton, Style::default().fg(app.theme.fg)),
                ]));

                // Prefix
                let pfx = app
                    .cfg
                    .games
                    .get(&g.app_name)
                    .and_then(|c| c.prefix_path.clone())
                    .unwrap_or_else(|| {
                        app.prefix_path(&g.app_name)
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| {
                                format!("{}/{}", app.cfg.default_prefix_path, g.app_name)
                            })
                    });
                lines.push(Line::from(vec![
                    Span::styled("Prefix:       ", Style::default().fg(app.theme.cyan)),
                    Span::styled(pfx, Style::default().fg(app.theme.fg)),
                ]));

                // Compact Flags row: GameMode [On]  MangoHud [Yes]  LSFG [Disabled]
                let gamemode = app
                    .cfg
                    .games
                    .get(&g.app_name)
                    .and_then(|c| c.gamemode)
                    .unwrap_or(app.cfg.default_gamemode);
                let mangohud = app
                    .cfg
                    .games
                    .get(&g.app_name)
                    .and_then(|c| c.mangohud)
                    .unwrap_or(app.cfg.default_mangohud);
                let lsfg = app
                    .cfg
                    .games
                    .get(&g.app_name)
                    .and_then(|c| c.lsfg.clone())
                    .unwrap_or_else(|| app.cfg.lsfg_multiplier.clone());

                let is_lsfg_enabled = lsfg != "Disabled";
                lines.push(Line::from(vec![
                    Span::styled("Flags:        ", Style::default().fg(app.theme.cyan)),
                    Span::styled("GameMode [", Style::default().fg(app.theme.muted)),
                    Span::styled(
                        if gamemode { "On" } else { "Off" },
                        Style::default().fg(if gamemode {
                            app.theme.green
                        } else {
                            app.theme.muted
                        }),
                    ),
                    Span::styled("]  MangoHud [", Style::default().fg(app.theme.muted)),
                    Span::styled(
                        if mangohud { "Yes" } else { "No" },
                        Style::default().fg(if mangohud {
                            app.theme.green
                        } else {
                            app.theme.muted
                        }),
                    ),
                    Span::styled("]  LSFG [", Style::default().fg(app.theme.muted)),
                    Span::styled(
                        lsfg,
                        Style::default().fg(if is_lsfg_enabled {
                            app.theme.accent
                        } else {
                            app.theme.muted
                        }),
                    ),
                    Span::styled("]", Style::default().fg(app.theme.muted)),
                ]));

                // Prerequisites / Anti-Cheat
                if let Some(d) = details {
                    if !d.prerequisites.is_empty() {
                        lines.push(Line::from(vec![
                            Span::styled("Prereqs:      ", Style::default().fg(app.theme.cyan)),
                            Span::styled(
                                d.prerequisites.join(", "),
                                Style::default().fg(app.theme.fg),
                            ),
                        ]));
                    }
                    if let Some(cs) = d.cloud_saves {
                        let cs_text = if cs {
                            match &d.cloud_save_folder {
                                Some(f) => format!("Supported ({f})"),
                                None => "Supported".into(),
                            }
                        } else {
                            "Not supported".into()
                        };
                        lines.push(Line::from(vec![
                            Span::styled("Cloud Saves:  ", Style::default().fg(app.theme.cyan)),
                            Span::styled(
                                cs_text,
                                Style::default().fg(if cs {
                                    app.theme.green
                                } else {
                                    app.theme.muted
                                }),
                            ),
                        ]));
                    }
                    if !d.installed_dlc.is_empty() {
                        lines.push(Line::from(vec![
                            Span::styled("DLCs (inst):  ", Style::default().fg(app.theme.cyan)),
                            Span::styled(
                                d.installed_dlc.join(", "),
                                Style::default().fg(app.theme.fg),
                            ),
                        ]));
                    }
                }
            } else {
                // UNINSTALLED GAME VIEW:
                let is_active_dl =
                    app.active_install.as_ref().map(|i| &i.app_name) == Some(&g.app_name);
                let queue_pos = app.install_queue.iter().position(|q| q == &g.app_name);

                if is_active_dl {
                    let inst = app.active_install.as_ref().unwrap();
                    lines.push(Line::from(vec![
                        Span::styled("Status:       ", Style::default().fg(app.theme.cyan)),
                        Span::styled(
                            format!("Downloading ({:.1}%)", inst.progress.percentage),
                            Style::default()
                                .fg(app.theme.cyan)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]));

                    let bar_width = 24;
                    let filled =
                        ((inst.progress.percentage / 100.0) * bar_width as f32).round() as usize;
                    let filled = filled.min(bar_width);
                    let empty = bar_width - filled;
                    let bar_str = format!(
                        "[{}{}] {:.1}%",
                        "█".repeat(filled),
                        "░".repeat(empty),
                        inst.progress.percentage
                    );
                    lines.push(Line::from(vec![
                        Span::styled("Progress:     ", Style::default().fg(app.theme.cyan)),
                        Span::styled(
                            bar_str,
                            Style::default()
                                .fg(app.theme.accent)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]));

                    lines.push(Line::from(vec![
                        Span::styled("Speed / ETA:  ", Style::default().fg(app.theme.cyan)),
                        Span::styled(
                            format!(
                                "{} • ETA: {}",
                                inst.progress.speed_str, inst.progress.eta_str
                            ),
                            Style::default().fg(app.theme.fg),
                        ),
                    ]));

                    if inst.progress.downloaded_bytes > 0 || inst.progress.total_bytes > 0 {
                        let dl_bytes = prefix::fmt_size(inst.progress.downloaded_bytes);
                        let total_bytes = if inst.progress.total_bytes > 0 {
                            prefix::fmt_size(inst.progress.total_bytes)
                        } else {
                            "Unknown".to_string()
                        };
                        lines.push(Line::from(vec![
                            Span::styled("Downloaded:   ", Style::default().fg(app.theme.cyan)),
                            Span::styled(
                                format!("{dl_bytes} / {total_bytes}"),
                                Style::default().fg(app.theme.fg),
                            ),
                        ]));
                    }

                    lines.push(Line::from(vec![
                        Span::styled("Stage:        ", Style::default().fg(app.theme.cyan)),
                        Span::styled(
                            &inst.progress.status_stage,
                            Style::default().fg(app.theme.yellow),
                        ),
                    ]));

                    lines.push(Line::from(vec![
                        Span::styled("Actions:      ", Style::default().fg(app.theme.cyan)),
                        Span::styled(
                            "[c] Cancel Download",
                            Style::default()
                                .fg(app.theme.red)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]));
                    lines.push(Line::from(""));
                } else if let Some(pos) = queue_pos {
                    lines.push(Line::from(vec![
                        Span::styled("Status:       ", Style::default().fg(app.theme.cyan)),
                        Span::styled(
                            format!("Queued for download (position #{})", pos + 1),
                            Style::default()
                                .fg(app.theme.yellow)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]));
                    lines.push(Line::from(vec![
                        Span::styled("Actions:      ", Style::default().fg(app.theme.cyan)),
                        Span::styled("[c] Remove from queue", Style::default().fg(app.theme.red)),
                    ]));
                    lines.push(Line::from(""));
                }

                // ProtonDB Rating
                let (pdb_lbl, pdb_val) =
                    protondb_badge(details.and_then(|d| d.protondb_tier.as_deref()), &app.theme);
                lines.push(Line::from(vec![pdb_lbl, pdb_val]));

                if let Some(d) = details {
                    if let Some(dl) = d.download_size {
                        lines.push(Line::from(vec![
                            Span::styled("Download:     ", Style::default().fg(app.theme.cyan)),
                            Span::styled(
                                prefix::fmt_size(dl),
                                Style::default().fg(app.theme.yellow),
                            ),
                        ]));
                    } else {
                        lines.push(Line::from(vec![
                            Span::styled("Download:     ", Style::default().fg(app.theme.cyan)),
                            Span::styled(
                                "Fetching game information...",
                                Style::default().fg(app.theme.muted),
                            ),
                        ]));
                    }

                    if let Some(cs) = d.cloud_saves {
                        let cs_text = if cs { "Supported" } else { "Not supported" };
                        lines.push(Line::from(vec![
                            Span::styled("Cloud Saves:  ", Style::default().fg(app.theme.cyan)),
                            Span::styled(
                                cs_text,
                                Style::default().fg(if cs {
                                    app.theme.green
                                } else {
                                    app.theme.muted
                                }),
                            ),
                        ]));
                    }

                    if !d.prerequisites.is_empty() {
                        lines.push(Line::from(vec![
                            Span::styled("Prereqs:      ", Style::default().fg(app.theme.cyan)),
                            Span::styled(
                                d.prerequisites.join(", "),
                                Style::default().fg(app.theme.fg),
                            ),
                        ]));
                    }

                    if let Some(dev) = &d.developer {
                        lines.push(Line::from(vec![
                            Span::styled("Developer:    ", Style::default().fg(app.theme.cyan)),
                            Span::styled(dev, Style::default().fg(app.theme.fg)),
                        ]));
                    }

                    if !d.genres.is_empty() {
                        let non_generic: Vec<&str> = d
                            .genres
                            .iter()
                            .filter(|x| *x != "games" && *x != "applications" && *x != "public")
                            .map(|s| s.as_str())
                            .collect();
                        if !non_generic.is_empty() {
                            lines.push(Line::from(vec![
                                Span::styled("Genre:        ", Style::default().fg(app.theme.cyan)),
                                Span::styled(
                                    non_generic.join(" / "),
                                    Style::default().fg(app.theme.fg),
                                ),
                            ]));
                        }
                    }
                } else {
                    lines.push(Line::from(vec![
                        Span::styled("Download:     ", Style::default().fg(app.theme.cyan)),
                        Span::styled(
                            "Fetching game information...",
                            Style::default().fg(app.theme.muted),
                        ),
                    ]));
                }
            }

            // Description
            lines.push(Line::from(""));
            lines.push(Line::from(vec![Span::styled(
                "Description:",
                Style::default()
                    .fg(app.theme.accent)
                    .add_modifier(Modifier::BOLD),
            )]));

            if let Some(desc) = details.and_then(|d| d.description.as_deref()) {
                // Word wrap description to area width
                let max_w = (area.width.saturating_sub(4) as usize).max(20);
                for wrapped in wrap_text(desc, max_w) {
                    lines.push(Line::from(Span::styled(
                        wrapped,
                        Style::default().fg(app.theme.fg),
                    )));
                }
            } else {
                lines.push(Line::from(Span::styled(
                    "N/A",
                    Style::default().fg(app.theme.muted),
                )));
            }
        }
        None => lines.push(Line::from(Span::styled(
            "No games match.",
            Style::default().fg(app.theme.muted),
        ))),
    }

    let visible_height = area.height.saturating_sub(2);
    let total_lines = lines.len() as u16;
    let max_scroll = total_lines.saturating_sub(visible_height);
    let scroll_y = app.detail_scroll.min(max_scroll);

    let title_text = if max_scroll > 0 {
        format!(
            " Game Details [J/K scroll ({}/{})] ",
            scroll_y + 1,
            max_scroll + 1
        )
    } else {
        " Game Details ".to_string()
    };

    f.render_widget(
        Paragraph::new(lines).scroll((scroll_y, 0)).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.theme.muted))
                .style(Style::default().bg(app.theme.bg).fg(app.theme.fg))
                .title(Span::styled(
                    title_text,
                    Style::default().fg(app.theme.accent),
                )),
        ),
        area,
    );
}

fn wrap_text(text: &str, max_width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        let mut cur = String::new();
        for word in paragraph.split_whitespace() {
            if cur.len() + word.len() + 1 > max_width && !cur.is_empty() {
                lines.push(cur);
                cur = String::new();
            }
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(word);
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
    }
    lines
}

fn draw_install_popup(
    f: &mut Frame,
    app: &App,
    prog: &crate::install::InstallProgress,
    area: Rect,
) {
    let r = centered(area, 68.min(area.width - 4), 14.min(area.height - 4));
    f.render_widget(Clear, r);

    let perc = prog.percentage.clamp(0.0, 100.0);
    let bar_width = 38usize;
    let filled = ((perc / 100.0) * bar_width as f32) as usize;
    let empty = bar_width.saturating_sub(filled);
    let bar_str = format!("{}{}", "█".repeat(filled), "░".repeat(empty));

    let size_str = if prog.downloaded_bytes > 0 && prog.total_bytes > 0 {
        format!(
            "{} / {}",
            prefix::fmt_size(prog.downloaded_bytes),
            prefix::fmt_size(prog.total_bytes)
        )
    } else if prog.downloaded_bytes > 0 {
        prefix::fmt_size(prog.downloaded_bytes)
    } else {
        "Starting download...".to_string()
    };

    let text = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Status: ", Style::default().fg(app.theme.cyan)),
            Span::styled(
                &prog.status_stage,
                Style::default()
                    .fg(app.theme.green)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(bar_str, Style::default().fg(app.theme.accent)),
            Span::styled(
                format!("  {:.1}%", perc),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Progress: ", Style::default().fg(app.theme.cyan)),
            Span::styled(size_str, Style::default().fg(app.theme.fg)),
        ]),
        Line::from(vec![
            Span::styled("  Speed:    ", Style::default().fg(app.theme.cyan)),
            Span::styled(&prog.speed_str, Style::default().fg(app.theme.yellow)),
            Span::styled("    ETA: ", Style::default().fg(app.theme.cyan)),
            Span::styled(&prog.eta_str, Style::default().fg(app.theme.fg)),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "  [q] cancel   [Esc] cancel",
            Style::default().fg(app.theme.muted),
        )]),
    ];

    let title_str = format!("INSTALLING: {}", prog.title);
    let block = solid_block(&title_str, &app.theme);
    f.render_widget(Paragraph::new(text).block(block), r);
}

fn draw_menu_popup(f: &mut Frame, app: &App, area: Rect) {
    let (title, items, idx) = match app.mode() {
        AppMode::Menu(m) => (m.title.as_str(), &m.items, m.idx),
        _ => return,
    };
    let h = (items.len() as u16 + 4).min(area.height - 4).max(6);
    let r = centered(area, 60.min(area.width - 4), h);
    f.render_widget(Clear, r);

    let list_items: Vec<ListItem> = items
        .iter()
        .enumerate()
        .map(|(i, (label, _))| {
            let is_sel = i == idx;
            let style = if is_sel {
                Style::default()
                    .bg(app.theme.accent)
                    .fg(app.theme.on_accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(app.theme.fg)
            };
            ListItem::new(Line::from(Span::styled(format!("  {label}"), style)))
        })
        .collect();

    let mut state = ratatui::widgets::ListState::default();
    state.select(Some(idx));
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.accent))
        .style(Style::default().bg(app.theme.bg2).fg(app.theme.fg))
        .title(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ))
        .title_bottom(Span::styled(
            " [h/l · ←/→] Adjust  [Enter] Select  [Esc] Back ",
            Style::default().fg(app.theme.cyan),
        ));

    f.render_stateful_widget(
        List::new(list_items)
            .block(block)
            .highlight_symbol("> ")
            .highlight_style(
                Style::default()
                    .bg(app.theme.accent)
                    .fg(app.theme.on_accent)
                    .add_modifier(Modifier::BOLD),
            ),
        r,
        &mut state,
    );
}

fn draw_confirm_popup(f: &mut Frame, app: &App, area: Rect) {
    let lines = match app.mode() {
        AppMode::Confirm { lines, .. } => lines,
        _ => return,
    };
    let h = (lines.len() as u16 + 5).min(area.height - 4).max(7);
    let r = centered(area, 64.min(area.width - 4), h);
    f.render_widget(Clear, r);
    let mut text: Vec<Line> = lines
        .iter()
        .map(|l| Line::from(Span::styled(l.clone(), Style::default().fg(app.theme.fg))))
        .collect();
    text.push(Line::from(""));
    text.push(Line::from(vec![
        Span::styled(
            "[y] Yes",
            Style::default()
                .fg(app.theme.green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("    "),
        Span::styled("[n] Cancel", Style::default().fg(app.theme.red)),
    ]));
    f.render_widget(
        Paragraph::new(text).block(solid_block("Confirm", &app.theme)),
        r,
    );
}

fn draw_input_popup(f: &mut Frame, app: &App, area: Rect) {
    let (title, buf) = match app.mode() {
        AppMode::Input { title, buf, .. } => (title.as_str(), buf.as_str()),
        _ => return,
    };
    let r = centered(area, 68.min(area.width - 4), 7);
    f.render_widget(Clear, r);
    let text = vec![
        Line::from(Span::styled(
            format!("> {buf}"),
            Style::default().fg(app.theme.fg),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Enter save · Esc cancel",
            Style::default().fg(app.theme.muted),
        )),
    ];
    f.render_widget(
        Paragraph::new(text).block(solid_block(title, &app.theme)),
        r,
    );
}

fn draw_details_popup(f: &mut Frame, app: &App, area: Rect) {
    let rows = match app.mode() {
        AppMode::Details(a) => app.detail_for(a),
        _ => return,
    };
    let h = (rows.len() as u16 + 5).min(area.height - 4).max(8);
    let r = centered(area, 72.min(area.width - 4), h);
    f.render_widget(Clear, r);
    let mut text: Vec<Line> = rows
        .into_iter()
        .map(|(k, v)| {
            Line::from(vec![
                Span::styled(format!("{k:16}: "), Style::default().fg(app.theme.cyan)),
                Span::styled(v, Style::default().fg(app.theme.fg)),
            ])
        })
        .collect();
    text.push(Line::from(""));
    text.push(Line::from(Span::styled(
        "Esc back",
        Style::default().fg(app.theme.muted),
    )));
    f.render_widget(
        Paragraph::new(text).block(solid_block("Game Details", &app.theme)),
        r,
    );
}

fn draw_help_popup(f: &mut Frame, app: &App, area: Rect) {
    let r = centered(area, 64.min(area.width - 4), 23.min(area.height - 4));
    f.render_widget(Clear, r);
    let text = vec![
        Line::from(vec![
            Span::styled("j / k, Down / Up  ", Style::default().fg(app.theme.cyan)),
            Span::raw("Navigate one game"),
        ]),
        Line::from(vec![
            Span::styled("J / K, Shift+j / k", Style::default().fg(app.theme.cyan)),
            Span::raw("Scroll description and game info"),
        ]),
        Line::from(vec![
            Span::styled("Ctrl+d / Ctrl+u   ", Style::default().fg(app.theme.cyan)),
            Span::raw("Page down / Page up"),
        ]),
        Line::from(vec![
            Span::styled("g / G / gg        ", Style::default().fg(app.theme.cyan)),
            Span::raw("Jump to first / last game"),
        ]),
        Line::from(vec![
            Span::styled("Tab               ", Style::default().fg(app.theme.cyan)),
            Span::raw("Toggle selection (multi-select)"),
        ]),
        Line::from(vec![
            Span::styled("f                 ", Style::default().fg(app.theme.cyan)),
            Span::raw("Cycle filters (All / Installed / Available)"),
        ]),
        Line::from(vec![
            Span::styled("Enter             ", Style::default().fg(app.theme.cyan)),
            Span::raw("Launch options (installed) / Install (uninstalled)"),
        ]),
        Line::from(vec![
            Span::styled("h / l, ← / →      ", Style::default().fg(app.theme.cyan)),
            Span::raw("Adjust/toggle in menus (MangoHud, LSFG, etc)"),
        ]),
        Line::from(vec![
            Span::styled("u                 ", Style::default().fg(app.theme.cyan)),
            Span::raw("Update / Repair selected"),
        ]),
        Line::from(vec![
            Span::styled("d                 ", Style::default().fg(app.theme.cyan)),
            Span::raw("Delete game menu (installed)"),
        ]),
        Line::from(vec![
            Span::styled("/                 ", Style::default().fg(app.theme.cyan)),
            Span::raw("Search library (Esc clears)"),
        ]),
        Line::from(vec![
            Span::styled("r                 ", Style::default().fg(app.theme.cyan)),
            Span::raw("Refresh library from Epic"),
        ]),
        Line::from(vec![
            Span::styled("s                 ", Style::default().fg(app.theme.cyan)),
            Span::raw("Settings (Proton, GameMode, LSFG, paths)"),
        ]),
        Line::from(vec![
            Span::styled("?                 ", Style::default().fg(app.theme.cyan)),
            Span::raw("Show this help dialog"),
        ]),
        Line::from(vec![
            Span::styled("q / Esc           ", Style::default().fg(app.theme.cyan)),
            Span::raw("Back / Quit"),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Dedicated Epic Prefixes: ~/.local/share/egs/prefixes/<appid>/",
            Style::default().fg(app.theme.muted),
        )),
        Line::from(Span::styled(
            "Alt+G Menu Integration: ~/.config/rofi/epic-games.list",
            Style::default().fg(app.theme.muted),
        )),
    ];
    f.render_widget(
        Paragraph::new(text).block(solid_block("Keyboard Navigation & Controls", &app.theme)),
        r,
    );
}

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
    let header = Paragraph::new(vec![Line::from(vec![
        Span::styled(" Epic Games ", title_style(app)),
        Span::styled(count_text, Style::default().fg(app.theme.fg)),
        Span::styled(
            format!("      {}", app.status),
            Style::default().fg(app.theme.yellow),
        ),
    ])])
    .block(
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
        .visible_games()
        .iter()
        .enumerate()
        .map(|(i, g)| {
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

            let line = Line::from(vec![
                Span::styled(check_str, check_style),
                Span::styled(inst_str, inst_style),
                Span::styled(g.title.clone(), title_style),
            ]);
            ListItem::new(line)
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

fn draw_detail(f: &mut Frame, app: &App, area: Rect) {
    let mut lines = Vec::new();
    match app.visible_games().get(app.selected) {
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

            // Installed State
            let state_span = if g.installed {
                Span::styled(
                    "Installed",
                    Style::default()
                        .fg(app.theme.green)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Span::styled("Not Installed", Style::default().fg(app.theme.muted))
            };
            lines.push(Line::from(vec![
                Span::styled("State:        ", Style::default().fg(app.theme.cyan)),
                state_span,
            ]));

            // App ID
            lines.push(Line::from(vec![
                Span::styled("App ID:       ", Style::default().fg(app.theme.cyan)),
                Span::styled(&g.app_name, Style::default().fg(app.theme.fg)),
            ]));

            // Version & Build ID
            let ver = g
                .version
                .as_deref()
                .or_else(|| details.and_then(|d| d.version.as_deref()))
                .unwrap_or("N/A");
            let mut ver_str = ver.to_string();
            if let Some(bid) = details.and_then(|d| d.build_id.as_deref()) {
                ver_str.push_str(&format!(" (Build: {bid})"));
            }
            lines.push(Line::from(vec![
                Span::styled("Version:      ", Style::default().fg(app.theme.cyan)),
                Span::styled(ver_str, Style::default().fg(app.theme.fg)),
            ]));

            // Platform
            if let Some(plt) = details.and_then(|d| d.platform.as_deref()) {
                lines.push(Line::from(vec![
                    Span::styled("Platform:     ", Style::default().fg(app.theme.cyan)),
                    Span::styled(plt, Style::default().fg(app.theme.fg)),
                ]));
            }

            // Sizes
            if let Some(d) = details {
                if let Some(dl) = d.download_size {
                    lines.push(Line::from(vec![
                        Span::styled("Download:     ", Style::default().fg(app.theme.cyan)),
                        Span::styled(prefix::fmt_size(dl), Style::default().fg(app.theme.yellow)),
                    ]));
                } else if !g.installed {
                    lines.push(Line::from(vec![
                        Span::styled("Download:     ", Style::default().fg(app.theme.cyan)),
                        Span::styled(
                            "Fetching game information...",
                            Style::default().fg(app.theme.muted),
                        ),
                    ]));
                }

                if let Some(inst) = d.installed_size {
                    lines.push(Line::from(vec![
                        Span::styled("Installed:    ", Style::default().fg(app.theme.cyan)),
                        Span::styled(prefix::fmt_size(inst), Style::default().fg(app.theme.fg)),
                    ]));
                }

                // Launch Exe
                if let Some(exe) = &d.launch_exe {
                    lines.push(Line::from(vec![
                        Span::styled("Executable:   ", Style::default().fg(app.theme.cyan)),
                        Span::styled(exe, Style::default().fg(app.theme.fg)),
                    ]));
                }

                // Cloud Saves
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
                            Style::default().fg(if cs { app.theme.green } else { app.theme.muted }),
                        ),
                    ]));
                }

                // Prerequisites / Anti-Cheat
                if !d.prerequisites.is_empty() {
                    lines.push(Line::from(vec![
                        Span::styled("Prereqs:      ", Style::default().fg(app.theme.cyan)),
                        Span::styled(
                            d.prerequisites.join(", "),
                            Style::default().fg(app.theme.fg),
                        ),
                    ]));
                }

                // DLCs
                if !d.installed_dlc.is_empty() {
                    lines.push(Line::from(vec![
                        Span::styled("DLCs (inst):  ", Style::default().fg(app.theme.cyan)),
                        Span::styled(
                            d.installed_dlc.join(", "),
                            Style::default().fg(app.theme.fg),
                        ),
                    ]));
                } else if !d.owned_dlc.is_empty() {
                    lines.push(Line::from(vec![
                        Span::styled("DLCs (owned): ", Style::default().fg(app.theme.cyan)),
                        Span::styled(
                            format!("{} owned", d.owned_dlc.len()),
                            Style::default().fg(app.theme.fg),
                        ),
                    ]));
                }
            }

            // Developer & Publisher
            if let Some(dev) = details.and_then(|d| d.developer.as_deref()) {
                lines.push(Line::from(vec![
                    Span::styled("Developer:    ", Style::default().fg(app.theme.cyan)),
                    Span::styled(dev, Style::default().fg(app.theme.fg)),
                ]));
            }
            if let Some(publ) = details.and_then(|d| d.publisher.as_deref()) {
                lines.push(Line::from(vec![
                    Span::styled("Publisher:    ", Style::default().fg(app.theme.cyan)),
                    Span::styled(publ, Style::default().fg(app.theme.fg)),
                ]));
            }

            // Release Date
            if let Some(date) =
                details.and_then(|d| d.release_date.as_deref().or(d.grant_date.as_deref()))
            {
                lines.push(Line::from(vec![
                    Span::styled("Release:      ", Style::default().fg(app.theme.cyan)),
                    Span::styled(date, Style::default().fg(app.theme.fg)),
                ]));
            }

            // Genres
            if let Some(genres) = details.map(|d| &d.genres) {
                if !genres.is_empty() {
                    lines.push(Line::from(vec![
                        Span::styled("Genre:        ", Style::default().fg(app.theme.cyan)),
                        Span::styled(genres.join(" / "), Style::default().fg(app.theme.fg)),
                    ]));
                }
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

            // Launch Options
            if let Some(args) = app
                .cfg
                .games
                .get(&g.app_name)
                .and_then(|c| c.launch_args.as_deref())
            {
                lines.push(Line::from(vec![
                    Span::styled("Launch Args:  ", Style::default().fg(app.theme.cyan)),
                    Span::styled(args, Style::default().fg(app.theme.fg)),
                ]));
            }

            // GameMode
            let gamemode = app
                .cfg
                .games
                .get(&g.app_name)
                .and_then(|c| c.gamemode)
                .unwrap_or(app.cfg.default_gamemode);
            lines.push(Line::from(vec![
                Span::styled("GameMode:     ", Style::default().fg(app.theme.cyan)),
                Span::styled(
                    if gamemode { "On" } else { "Off" },
                    Style::default().fg(if gamemode {
                        app.theme.green
                    } else {
                        app.theme.muted
                    }),
                ),
            ]));

            // MangoHud
            let mangohud = app
                .cfg
                .games
                .get(&g.app_name)
                .and_then(|c| c.mangohud)
                .unwrap_or(app.cfg.default_mangohud);
            lines.push(Line::from(vec![
                Span::styled("MangoHud:     ", Style::default().fg(app.theme.cyan)),
                Span::styled(
                    if mangohud { "Yes" } else { "No" },
                    Style::default().fg(if mangohud {
                        app.theme.green
                    } else {
                        app.theme.muted
                    }),
                ),
            ]));

            // LSFG Frame Gen
            let lsfg = app
                .cfg
                .games
                .get(&g.app_name)
                .and_then(|c| c.lsfg.clone())
                .unwrap_or_else(|| app.cfg.lsfg_multiplier.clone());
            lines.push(Line::from(vec![
                Span::styled("LSFG Frame:   ", Style::default().fg(app.theme.cyan)),
                Span::styled(
                    lsfg.clone(),
                    Style::default().fg(if lsfg != "Disabled" {
                        app.theme.accent
                    } else {
                        app.theme.muted
                    }),
                ),
            ]));

            // Install Location
            if let Some(p) = &g.install_path {
                lines.push(Line::from(vec![
                    Span::styled("Install Path: ", Style::default().fg(app.theme.cyan)),
                    Span::styled(p, Style::default().fg(app.theme.fg)),
                ]));
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

    f.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(app.theme.muted))
                .style(Style::default().bg(app.theme.bg).fg(app.theme.fg))
                .title(Span::styled(
                    " Game Details ",
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
    let r = centered(area, 64.min(area.width - 4), 22.min(area.height - 4));
    f.render_widget(Clear, r);
    let text = vec![
        Line::from(vec![
            Span::styled("j / k, Down / Up  ", Style::default().fg(app.theme.cyan)),
            Span::raw("Navigate one game"),
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

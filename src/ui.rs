//! All rendering for legendary-tui. Read-only over App; no side effects, no animation.
//! Follows the desktop theme: accent fills, solid backgrounds, and high-contrast text.

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, List, ListItem, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState,
    },
    Frame,
};

use crate::app::{App, AppMode};
use crate::prefix;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Breakpoint {
    Full,
    Medium,
    Compact,
}

#[derive(Debug, Clone)]
pub struct LayoutAreas {
    pub outer: Rect,
    #[allow(dead_code)]
    pub inner: Rect,
    #[allow(dead_code)]
    pub body: Rect,
    pub list: Rect,
    pub divider: Option<Rect>,
    pub detail: Option<Rect>,
    pub cover: Option<Rect>,
    #[allow(dead_code)]
    pub cover_source: Option<Rect>,
    pub info: Option<Rect>,
    #[allow(dead_code)]
    pub facts: Option<Rect>,
    #[allow(dead_code)]
    pub description: Option<Rect>,
    pub footer: Rect,
    #[allow(dead_code)]
    pub breakpoint: Breakpoint,
}

pub fn should_show_checkboxes(selected_count: usize) -> bool {
    selected_count > 0
}

#[allow(dead_code)]
pub fn footer_hints(is_installed: bool, has_active_job: bool) -> &'static str {
    if has_active_job {
        "j/k move   Enter install/options   x cancel   o epic   / search   f filter   r refresh   ? help   q quit"
    } else if is_installed {
        "j/k move   Enter options   o epic   / search   f filter   r refresh   ? help   q quit"
    } else {
        "j/k move   Enter install   o epic   / search   f filter   r refresh   ? help   q quit"
    }
}

pub fn truncate_with_ellipsis(s: &str, max_w: usize) -> String {
    if s.width() <= max_w {
        return s.to_string();
    }
    if max_w == 0 {
        return String::new();
    }
    let target_w = max_w.saturating_sub(1);
    let mut out = String::new();
    for c in s.chars() {
        let cw = UnicodeWidthChar::width(c).unwrap_or(1);
        if out.width() + cw > target_w {
            break;
        }
        out.push(c);
    }
    out.push('…');
    out
}

pub fn wrap_title(title: &str, max_width: usize) -> Vec<String> {
    if title.width() <= max_width {
        return vec![title.to_string()];
    }
    let words: Vec<&str> = title.split_whitespace().collect();
    let mut lines = Vec::new();
    let mut cur = String::new();

    for w in &words {
        if !cur.is_empty() && cur.width() + 1 + w.width() > max_width {
            lines.push(cur);
            cur = String::new();
            if lines.len() == 2 {
                break;
            }
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(w);
    }
    if lines.len() < 2 && !cur.is_empty() {
        lines.push(cur);
    }
    let total_words = words.len();
    let words_in_lines: usize = lines.iter().map(|l| l.split_whitespace().count()).sum();
    let has_overflow = words_in_lines < total_words;

    if lines.len() == 2 {
        if has_overflow {
            if lines[1].width() < max_width {
                lines[1].push('…');
            } else {
                lines[1] = truncate_with_ellipsis(&lines[1], max_width);
                if !lines[1].ends_with('…') {
                    lines[1] = truncate_with_ellipsis(&lines[1], max_width.saturating_sub(1));
                    lines[1].push('…');
                }
            }
        } else if lines[1].width() > max_width {
            lines[1] = truncate_with_ellipsis(&lines[1], max_width);
        }
    } else if lines.len() == 1 && (has_overflow || lines[0].width() > max_width) {
        lines[0] = truncate_with_ellipsis(&lines[0], max_width);
    }
    lines
}

pub fn compute_cover_rect(container: Rect, font_size: (u16, u16)) -> Rect {
    let (cw, ch) = if font_size.0 > 0 && font_size.1 > 0 {
        font_size
    } else {
        (10, 20)
    };

    if container.width < 10 || container.height <= 2 {
        return Rect::default();
    }

    // Max width = 40% of right pane inner width, capped at 40 cols
    let max_w = (container.width * 40 / 100)
        .clamp(1, 40)
        .min(container.width.saturating_sub(10).max(1));
    let max_h = container.height.saturating_sub(1);

    // Target aspect ratio is 2:3 physical pixels:
    // (w * cw) / (h * ch) == 2 / 3
    // h = round((w * cw * 3) / (2 * ch))
    let mut w = max_w;
    let mut h = ((w as u32 * cw as u32 * 3 + ch as u32) / (2 * ch as u32)) as u16;

    if h > max_h {
        h = max_h.max(1);
        w = (((h as u32 * ch as u32 * 2 + (cw as u32 * 3 / 2)) / (3 * cw as u32)) as u16)
            .min(max_w)
            .max(1);
    }

    Rect {
        x: container.x,
        y: container.y,
        width: w,
        height: h,
    }
}

pub fn compute_layout(
    area: Rect,
    _searching_or_status: bool,
    font_size: (u16, u16),
) -> LayoutAreas {
    let outer = area;
    if area.width < 10 || area.height < 6 {
        return LayoutAreas {
            outer,
            inner: area,
            body: area,
            list: area,
            divider: None,
            detail: None,
            cover: None,
            cover_source: None,
            info: None,
            facts: None,
            description: None,
            footer: Rect::default(),
            breakpoint: Breakpoint::Compact,
        };
    }

    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };
    let footer_h = if inner.height >= 6 { 2 } else { 1 };
    let body_h = inner.height.saturating_sub(footer_h);
    let body = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: body_h,
    };
    let footer = Rect {
        x: inner.x,
        y: inner.y + body_h,
        width: inner.width,
        height: footer_h,
    };

    if area.width < 70 {
        LayoutAreas {
            outer,
            inner,
            body,
            list: body,
            divider: None,
            detail: None,
            cover: None,
            cover_source: None,
            info: None,
            facts: None,
            description: None,
            footer,
            breakpoint: Breakpoint::Compact,
        }
    } else if area.width < 100 {
        let list_w = (body.width * 35 / 100).clamp(28, body.width.saturating_sub(30));
        let list = Rect {
            x: body.x,
            y: body.y,
            width: list_w,
            height: body.height,
        };
        let divider = Some(Rect {
            x: body.x + list_w,
            y: body.y,
            width: 1,
            height: body.height,
        });
        let right_x = body.x + list_w + 1;
        let right_w = body.width.saturating_sub(list_w + 1);
        let detail = Some(Rect {
            x: right_x,
            y: body.y,
            width: right_w,
            height: body.height,
        });
        let info = Some(Rect {
            x: right_x,
            y: body.y,
            width: right_w,
            height: body.height,
        });
        LayoutAreas {
            outer,
            inner,
            body,
            list,
            divider,
            detail,
            cover: None,
            cover_source: None,
            info,
            facts: info,
            description: info,
            footer,
            breakpoint: Breakpoint::Medium,
        }
    } else {
        let list_w = (body.width * 35 / 100).clamp(28, body.width.saturating_sub(55));
        let list = Rect {
            x: body.x,
            y: body.y,
            width: list_w,
            height: body.height,
        };
        let divider = Some(Rect {
            x: body.x + list_w,
            y: body.y,
            width: 1,
            height: body.height,
        });
        let right_x = body.x + list_w + 1;
        let right_w = body.width.saturating_sub(list_w + 1);
        let detail = Some(Rect {
            x: right_x,
            y: body.y,
            width: right_w,
            height: body.height,
        });
        let cov_rect = compute_cover_rect(
            Rect {
                x: right_x,
                y: body.y,
                width: right_w,
                height: body.height,
            },
            font_size,
        );
        let gap = 2;
        let info_x = right_x + cov_rect.width + gap;
        let info_w = right_w.saturating_sub(cov_rect.width + gap);
        let info = Some(Rect {
            x: info_x,
            y: body.y,
            width: info_w,
            height: body.height,
        });
        LayoutAreas {
            outer,
            inner,
            body,
            list,
            divider,
            detail,
            cover: Some(cov_rect),
            cover_source: None,
            info,
            facts: info,
            description: info,
            footer,
            breakpoint: Breakpoint::Full,
        }
    }
}

#[allow(dead_code)]
pub fn format_footer_spans<'a>(hints_str: &str, theme: &'a crate::theme::Theme) -> Vec<Span<'a>> {
    let mut spans = Vec::new();
    spans.push(Span::raw(" "));
    let items = hints_str.split("   ");
    let mut first = true;
    for item in items {
        if !first {
            spans.push(Span::styled("   ", Style::default().fg(theme.muted)));
        }
        first = false;
        if let Some((k, v)) = item.split_once(' ') {
            spans.push(Span::styled(
                k.to_string(),
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                format!(" {v}"),
                Style::default().fg(theme.muted),
            ));
        } else {
            spans.push(Span::styled(
                item.to_string(),
                Style::default().fg(theme.muted),
            ));
        }
    }
    spans
}

pub fn build_responsive_footer_spans<'a>(
    is_installed: bool,
    cancel_hint: Option<&str>,
    available_width: usize,
    theme: &'a crate::theme::Theme,
) -> Vec<Span<'a>> {
    let enter_hint = if is_installed {
        ("Enter", "options")
    } else {
        ("Enter", "install")
    };

    let r_hint = ("r", "refresh");
    let f_hint = ("f", "filter");
    let o_hint = ("o", "epic");
    let search_hint = ("/", "search");

    let make_set = |extra_hints: &[(&'static str, &'static str)],
                    c_hint: Option<&str>|
     -> Vec<(&'static str, String)> {
        let mut ordered = Vec::new();
        ordered.push(("j/k", "move".to_string()));
        ordered.push((enter_hint.0, enter_hint.1.to_string()));
        if let Some(c) = c_hint {
            ordered.push(("x", c.to_string()));
        }
        for (k, v) in extra_hints {
            ordered.push((*k, v.to_string()));
        }
        ordered.push(("?", "help".to_string()));
        ordered.push(("q", "quit".to_string()));
        ordered
    };

    let calc_width = |items: &[(&'static str, String)]| -> usize {
        if items.is_empty() {
            return 0;
        }
        let content_len: usize = items.iter().map(|(k, v)| k.len() + 1 + v.len()).sum();
        let sep_len = (items.len().saturating_sub(1)) * 3;
        1 + content_len + sep_len
    };

    let candidate_sets = vec![
        // Level 0: all hints
        make_set(&[search_hint, f_hint, o_hint, r_hint], cancel_hint),
        // Level 1: drop r refresh
        make_set(&[search_hint, f_hint, o_hint], cancel_hint),
        // Level 2: drop f filter
        make_set(&[search_hint, o_hint], cancel_hint),
        // Level 3: drop o epic
        make_set(&[search_hint], cancel_hint),
        // Level 4: core only (drop / search)
        make_set(&[], cancel_hint),
    ];

    let mut chosen = candidate_sets.last().unwrap().clone();
    for set in &candidate_sets {
        if calc_width(set) <= available_width {
            chosen = set.clone();
            break;
        }
    }

    // If Level 4 still overflows available width and cancel hint is long, fallback to "cancel"
    if calc_width(&chosen) > available_width && cancel_hint.is_some() {
        let fallback = make_set(&[], Some("cancel"));
        if calc_width(&fallback) <= available_width {
            chosen = fallback;
        }
    }

    let mut spans = Vec::new();
    spans.push(Span::raw(" "));
    for (idx, (k, v)) in chosen.iter().enumerate() {
        if idx > 0 {
            spans.push(Span::styled("   ", Style::default().fg(theme.muted)));
        }
        spans.push(Span::styled(
            k.to_string(),
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!(" {v}"),
            Style::default().fg(theme.muted),
        ));
    }
    spans
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let searching_or_status = app.searching
        || !app.search.is_empty()
        || !app.status.is_empty()
        || app.active_install.is_some()
        || app.active_verify.is_some()
        || !app.install_queue.is_empty();
    let fs = app.picker.font_size();
    let layout = compute_layout(area, searching_or_status, (fs.width, fs.height));

    // Title for outer block:
    let count_text = if app.selected_games.is_empty() {
        format!("{}", app.games.len())
    } else {
        format!(
            "{} • {} selected",
            app.games.len(),
            app.selected_games.len()
        )
    };
    let title_str = format!(" Library [{}] ({count_text}) ", app.filter.label());

    let outer_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.theme.muted))
        .style(Style::default().bg(app.theme.bg).fg(app.theme.fg))
        .title(Span::styled(title_str, title_style(app)));
    f.render_widget(outer_block, layout.outer);

    // List
    draw_list(f, app, layout.list);

    // Divider + Scrollbar (single column)
    if let Some(div_rect) = layout.divider {
        let total_items = app.filtered_rows().len();
        if total_items > layout.list.height as usize {
            let mut scrollbar_state = ScrollbarState::new(total_items).position(app.selected);
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .thumb_symbol("█")
                .track_symbol(Some("│"))
                .thumb_style(Style::default().fg(app.theme.accent))
                .track_style(Style::default().fg(app.theme.muted))
                .begin_symbol(None)
                .end_symbol(None);
            f.render_stateful_widget(scrollbar, div_rect, &mut scrollbar_state);
        } else {
            let div_block = Block::default()
                .borders(Borders::LEFT)
                .border_style(Style::default().fg(app.theme.muted));
            f.render_widget(div_block, div_rect);
        }
    }

    // Detail (Cover + Facts + Description)
    if layout.detail.is_some() {
        draw_detail(f, app, &layout);
    }

    // Footer
    draw_footer(f, app, layout.footer);

    // Popups
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

pub fn compute_cancel_label(
    active_install: Option<(&str, &str)>,
    active_verify: Option<(&str, &str)>,
    current_app: Option<&str>,
) -> Option<String> {
    if let Some((app_name, title)) = active_install {
        if current_app == Some(app_name) {
            Some("cancel".to_string())
        } else {
            let t = truncate_with_ellipsis(title, 14);
            Some(format!("cancel {t}"))
        }
    } else if let Some((app_name, title)) = active_verify {
        if current_app == Some(app_name) {
            Some("cancel".to_string())
        } else {
            let t = truncate_with_ellipsis(title, 14);
            Some(format!("cancel {t}"))
        }
    } else {
        None
    }
}

pub fn compute_download_bar_line(
    is_selected_dl: bool,
    inst_title: &str,
    percentage: f32,
    speed_str: &str,
    eta_str: &str,
    queue_len: usize,
) -> Option<String> {
    if is_selected_dl {
        None
    } else {
        let q_str = if queue_len == 0 {
            String::new()
        } else {
            format!(" (+{queue_len} queued)")
        };
        let max_title_w = 24;
        let title_trunc = truncate_with_ellipsis(inst_title, max_title_w);
        Some(format!(
            "[↓ {} {:.1}% • {} • ETA {}]{}",
            title_trunc, percentage, speed_str, eta_str, q_str
        ))
    }
}

pub fn format_download_progress_bar(percentage: f32, bar_width: usize) -> (String, String) {
    let filled = ((percentage / 100.0) * bar_width as f32).round() as usize;
    let filled = filled.min(bar_width);
    let empty = bar_width - filled;
    ("━".repeat(filled), "─".repeat(empty))
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    if area.height == 0 || area.width == 0 {
        return;
    }

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
    let cancel_label = compute_cancel_label(
        app.active_install
            .as_ref()
            .map(|i| (i.app_name.as_str(), i.title.as_str())),
        app.active_verify
            .as_ref()
            .map(|v| (v.app_name.as_str(), v.title.as_str())),
        app.current().map(|g| g.app_name.as_str()),
    );

    let rows = if area.height > 1 {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Length(1)])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1)])
            .split(area)
    };

    // Row 0: hints on left, user on right
    let user_str = app.user_name.as_deref().unwrap_or("");
    let user_width = if user_str.is_empty() {
        0
    } else {
        (user_str.chars().count() as u16) + 4
    };
    let prefetch_info = app
        .artwork_mgr
        .prefetch_progress()
        .map(|(done, total)| format!("🎨 {done}/{total} "));
    let prefetch_width = prefetch_info
        .as_ref()
        .map(|s| s.chars().count() as u16)
        .unwrap_or(0);
    let right_width = user_width + prefetch_width;

    if right_width > 0 && rows[0].width > right_width + 20 {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(1), Constraint::Length(right_width)])
            .split(rows[0]);

        let left_width = cols[0].width as usize;
        let left_spans = build_responsive_footer_spans(
            is_installed,
            cancel_label.as_deref(),
            left_width,
            &app.theme,
        );
        let left_p = Paragraph::new(Line::from(left_spans));
        f.render_widget(left_p, cols[0]);

        let mut right_spans = Vec::new();
        if let Some(ref p_str) = prefetch_info {
            right_spans.push(Span::styled(
                p_str.clone(),
                Style::default().fg(app.theme.muted),
            ));
        }
        if !user_str.is_empty() {
            right_spans.push(Span::styled("👤 ", Style::default().fg(app.theme.accent)));
            right_spans.push(Span::styled(
                user_str,
                Style::default()
                    .fg(app.theme.accent)
                    .add_modifier(Modifier::BOLD),
            ));
            right_spans.push(Span::raw(" "));
        }
        let right_p = Paragraph::new(Line::from(right_spans)).alignment(Alignment::Right);
        f.render_widget(right_p, cols[1]);
    } else {
        let left_width = rows[0].width as usize;
        let left_spans = build_responsive_footer_spans(
            is_installed,
            cancel_label.as_deref(),
            left_width,
            &app.theme,
        );
        let p = Paragraph::new(Line::from(left_spans));
        f.render_widget(p, rows[0]);
    }

    // Row 1 (if present): search or status/progress
    if rows.len() > 1 {
        let line = if app.searching {
            Line::from(vec![
                Span::styled(
                    " / ",
                    Style::default()
                        .fg(app.theme.accent)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    &app.search,
                    Style::default()
                        .fg(app.theme.fg)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("█", Style::default().fg(app.theme.accent)),
                Span::styled(
                    "   [Enter] accept · [Esc] cancel",
                    Style::default().fg(app.theme.muted),
                ),
            ])
        } else if !app.search.is_empty() {
            Line::from(vec![
                Span::styled(
                    " / ",
                    Style::default()
                        .fg(app.theme.accent)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    &app.search,
                    Style::default()
                        .fg(app.theme.fg)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "   [/] search · [Esc] clear",
                    Style::default().fg(app.theme.muted),
                ),
            ])
        } else if let Some(inst) = &app.active_install {
            let is_selected_dl = app
                .current()
                .map(|g| g.app_name == inst.app_name)
                .unwrap_or(false);
            if let Some(bar_str) = compute_download_bar_line(
                is_selected_dl,
                &inst.title,
                inst.progress.percentage,
                &inst.progress.speed_str,
                &inst.progress.eta_str,
                app.install_queue.len(),
            ) {
                Line::from(vec![
                    Span::styled(" ", Style::default()),
                    Span::styled(
                        bar_str,
                        Style::default()
                            .fg(app.theme.cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                ])
            } else {
                Line::from("")
            }
        } else if let Some(vfy) = &app.active_verify {
            Line::from(vec![
                Span::styled(" ", Style::default()),
                Span::styled(
                    format!(
                        "[✓ {} {:.1}% • {} checked • {}]",
                        vfy.title,
                        vfy.progress.percentage,
                        vfy.progress.files_checked_str(),
                        vfy.progress.speed_str
                    ),
                    Style::default()
                        .fg(app.theme.green)
                        .add_modifier(Modifier::BOLD),
                ),
            ])
        } else if !app.install_queue.is_empty() {
            Line::from(vec![
                Span::styled(" ", Style::default()),
                Span::styled(
                    format!("[{} games queued for download]", app.install_queue.len()),
                    Style::default()
                        .fg(app.theme.yellow)
                        .add_modifier(Modifier::BOLD),
                ),
            ])
        } else if !app.status.is_empty() {
            Line::from(vec![
                Span::styled(" ", Style::default()),
                Span::styled(&app.status, Style::default().fg(app.theme.yellow)),
            ])
        } else {
            Line::from("")
        };
        f.render_widget(Paragraph::new(line), rows[1]);
    }
}

fn draw_list(f: &mut Frame, app: &App, area: Rect) {
    let show_checks = should_show_checkboxes(app.selected_games.len());
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

                let mut spans = Vec::new();
                if show_checks {
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
                    spans.push(Span::styled(check_str, check_style));
                }

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
                spans.push(Span::styled(inst_str, inst_style));

                let title_style = if is_sel {
                    Style::default()
                        .fg(app.theme.on_accent)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(app.theme.fg)
                };

                let is_active_dl =
                    app.active_install.as_ref().map(|i| &i.app_name) == Some(&g.app_name);
                let queue_pos = app.install_queue.iter().position(|q| q == &g.app_name);

                let marker_info = if is_active_dl {
                    let perc = app
                        .active_install
                        .as_ref()
                        .map(|i| i.progress.percentage)
                        .unwrap_or(0.0);
                    let m_text = format!("↓ {:.0}%", perc);
                    let m_style = if is_sel {
                        Style::default()
                            .fg(app.theme.on_accent)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                            .fg(app.theme.accent)
                            .add_modifier(Modifier::BOLD)
                    };
                    Some((m_text, m_style))
                } else if queue_pos.is_some() {
                    let m_text = "queued".to_string();
                    let m_style = if is_sel {
                        Style::default()
                            .fg(app.theme.on_accent)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(app.theme.muted)
                    };
                    Some((m_text, m_style))
                } else if g.needs_update {
                    let m_text = "[UPDATE]".to_string();
                    let m_style = if is_sel {
                        Style::default()
                            .fg(app.theme.on_accent)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                            .fg(app.theme.yellow)
                            .add_modifier(Modifier::BOLD)
                    };
                    Some((m_text, m_style))
                } else {
                    None
                };

                let prefix_w = if show_checks { 4 } else { 0 } + 4; // 2 for highlight '> ', 2 for dot
                let marker_len = marker_info
                    .as_ref()
                    .map(|(s, _)| s.width() + 1)
                    .unwrap_or(0);
                let avail_title_w = (area.width as usize).saturating_sub(prefix_w + marker_len);
                let title_trunc = truncate_with_ellipsis(&g.title, avail_title_w);
                let title_w = title_trunc.width();
                spans.push(Span::styled(title_trunc, title_style));

                let pad_w = (area.width as usize).saturating_sub(prefix_w + title_w + marker_len);
                if pad_w > 0 {
                    spans.push(Span::raw(" ".repeat(pad_w)));
                }
                if let Some((m_text, m_style)) = marker_info {
                    spans.push(Span::styled(format!(" {m_text}"), m_style));
                }
                let line = Line::from(spans);
                ListItem::new(line)
            }
        })
        .collect();

    let mut state = ratatui::widgets::ListState::default();
    state.select(Some(app.selected));

    let list = List::new(items).highlight_symbol("> ").highlight_style(
        Style::default()
            .bg(app.theme.accent)
            .fg(app.theme.on_accent)
            .add_modifier(Modifier::BOLD),
    );
    f.render_stateful_widget(list, area, &mut state);
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
                    .fg(theme.yellow)
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

pub fn shorten_path(path: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let with_tilde = if !home.is_empty() && path.starts_with(&home) {
        format!("~{}", &path[home.len()..])
    } else {
        path.to_string()
    };

    if with_tilde.width() <= max_width {
        return with_tilde;
    }

    if max_width <= 3 {
        return "...".chars().take(max_width).collect();
    }

    let last_slash = with_tilde.rfind('/');
    let (prefix, tail) = match last_slash {
        Some(idx) => (&with_tilde[..idx], &with_tilde[idx..]),
        None => ("", with_tilde.as_str()),
    };

    let ellipsis = "...";
    let tail_w = tail.width();
    if tail_w + ellipsis.width() <= max_width {
        let avail_prefix_w = max_width - tail_w - ellipsis.width();
        let mut short_prefix = String::new();
        let mut cur_w = 0;
        for c in prefix.chars() {
            let cw = UnicodeWidthChar::width(c).unwrap_or(1);
            if cur_w + cw > avail_prefix_w {
                break;
            }
            short_prefix.push(c);
            cur_w += cw;
        }
        if short_prefix.len() < prefix.len() && short_prefix.contains('/') {
            if let Some(slash_idx) = short_prefix.rfind('/') {
                if slash_idx > 0 {
                    short_prefix.truncate(slash_idx);
                }
            }
        }
        if short_prefix.ends_with('/') {
            format!("{}{}{}", short_prefix, ellipsis, tail)
        } else if !short_prefix.is_empty() {
            format!("{}/...{}", short_prefix, tail)
        } else {
            format!("...{}", tail)
        }
    } else {
        let prefix_lead = if with_tilde.starts_with('~') {
            "~"
        } else {
            "/"
        };
        let avail_tail = max_width.saturating_sub(prefix_lead.len() + ellipsis.width());
        let tail_chars: Vec<char> = tail.chars().collect();
        let tail_slice: String = tail_chars
            .into_iter()
            .rev()
            .take(avail_tail)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        format!("{}{}{}", prefix_lead, ellipsis, tail_slice)
    }
}

pub fn format_enabled_flags(gamemode: bool, mangohud: bool, lsfg: &str) -> Vec<String> {
    let mut flags = Vec::new();
    if gamemode {
        flags.push("GameMode".to_string());
    }
    if mangohud {
        flags.push("MangoHud".to_string());
    }
    if lsfg != "Disabled" {
        flags.push(format!("LSFG ({lsfg})"));
    }
    if flags.is_empty() {
        vec!["None".to_string()]
    } else {
        flags
    }
}

pub fn should_show_description(desc: Option<&str>, title: &str) -> bool {
    let Some(d) = desc else {
        return false;
    };
    let trimmed_desc = d.trim();
    let trimmed_title = title.trim();
    if trimmed_desc.is_empty() {
        return false;
    }
    !trimmed_desc.eq_ignore_ascii_case(trimmed_title)
}

pub fn filter_rows_by_priority<'a>(
    mut rows: Vec<(&'static str, Vec<Span<'a>>)>,
    available_rows: usize,
) -> Vec<(&'static str, Vec<Span<'a>>)> {
    if rows.len() <= available_rows {
        return rows;
    }
    let is_uninstalled = rows.iter().any(|(l, v)| {
        *l == "Free space:"
            || (*l == "State:"
                && v.iter()
                    .any(|s| s.content == "Uninstalled" || s.content == "Not installed"))
    });
    let drop_order = if is_uninstalled {
        vec![
            "Requires:",
            "Executable:",
            "Owned since:",
            "DLC:",
            "Free space:",
        ]
    } else {
        vec!["Flags:", "Prefix:", "Executable:", "Prereqs:"]
    };
    for label in drop_order {
        if rows.len() <= available_rows {
            break;
        }
        if let Some(pos) = rows.iter().position(|(l, _)| *l == label) {
            rows.remove(pos);
        }
    }
    rows
}

fn draw_detail(f: &mut Frame, app: &mut App, layout: &LayoutAreas) {
    if let Some(cov_rect) = layout.cover {
        draw_cover(f, app, Some(cov_rect));
    }
    if let Some(info_rect) = layout.info {
        draw_info(f, app, info_rect);
    }
}

fn draw_info(f: &mut Frame, app: &App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let Some(g) = app.current() else {
        let p = Paragraph::new(Line::from(Span::styled(
            "No games match.",
            Style::default().fg(app.theme.muted),
        )));
        f.render_widget(p, area);
        return;
    };

    let details = app.current_details();
    let mut lines = Vec::new();

    // 1. Title (accent, bold, word-wrapped up to 2 lines, ellipsis if overflowing)
    let title_lines = wrap_title(&g.title, area.width as usize);
    for tl in title_lines {
        lines.push(Line::from(vec![Span::styled(
            tl,
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        )]));
    }

    // 2. Developer (dim) if present
    if let Some(dev) = details.and_then(|d| d.developer.as_deref()) {
        if !dev.is_empty() {
            lines.push(Line::from(vec![Span::styled(
                dev,
                Style::default().fg(app.theme.muted),
            )]));
        }
    }

    // Blank line
    lines.push(Line::from(""));

    // 3. Aligned label/value rows (only rows with data, label col width = longest label + 1)
    let mut rows: Vec<(&'static str, Vec<Span>)> = Vec::new();
    let mut show_install_hint = false;

    if g.installed {
        let mut state_spans = vec![Span::styled(
            "Installed",
            Style::default()
                .fg(app.theme.green)
                .add_modifier(Modifier::BOLD),
        )];
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
        rows.push(("State:", state_spans));

        let ver = g
            .version
            .as_deref()
            .or_else(|| details.and_then(|d| d.version.as_deref()))
            .unwrap_or("N/A");
        if ver != "N/A" {
            rows.push((
                "Version:",
                vec![Span::styled(ver, Style::default().fg(app.theme.fg))],
            ));
        }

        let pdb_tier = details
            .and_then(|d| d.protondb_tier.as_deref())
            .or_else(|| app.metadata_mgr.get_protondb_tier(&g.app_name));
        let (_, pdb_val) = protondb_badge(pdb_tier, &app.theme);
        rows.push(("ProtonDB:", vec![pdb_val]));

        if let Some(inst_sz) = details.and_then(|d| d.installed_size) {
            let ext = details.and_then(|d| d.external_activation.as_deref());
            let third = details.and_then(|d| d.third_party_managed.as_deref());
            let inst_str = prefix::fmt_game_size(inst_sz, ext, third);
            rows.push((
                "Installed:",
                vec![Span::styled(inst_str, Style::default().fg(app.theme.fg))],
            ));
        }

        if let Some(exe) = details.and_then(|d| d.launch_exe.as_deref()) {
            rows.push((
                "Executable:",
                vec![Span::styled(exe, Style::default().fg(app.theme.fg))],
            ));
        }

        let custom_proton = app
            .cfg
            .games
            .get(&g.app_name)
            .and_then(|c| c.proton.clone());
        let is_platinum = pdb_tier
            .map(|t| t.eq_ignore_ascii_case("platinum"))
            .unwrap_or(false);

        if let Some(p) = custom_proton {
            rows.push((
                "Proton:",
                vec![Span::styled(p, Style::default().fg(app.theme.fg))],
            ));
        } else if !is_platinum {
            rows.push((
                "Proton:",
                vec![Span::styled(
                    format!("Default ({})", app.cfg.default_proton),
                    Style::default().fg(app.theme.fg),
                )],
            ));
        }

        let pfx = app
            .cfg
            .games
            .get(&g.app_name)
            .and_then(|c| c.prefix_path.clone())
            .unwrap_or_else(|| {
                app.prefix_path(&g.app_name)
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| format!("{}/{}", app.cfg.default_prefix_path, g.app_name))
            });
        rows.push((
            "Prefix:",
            vec![Span::styled(pfx, Style::default().fg(app.theme.fg))],
        ));

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

        let flags_list = format_enabled_flags(gamemode, mangohud, &lsfg);
        let flag_spans: Vec<Span> = if flags_list == vec!["None"] {
            vec![Span::styled("None", Style::default().fg(app.theme.muted))]
        } else {
            let mut spans = Vec::new();
            for (idx, flag) in flags_list.iter().enumerate() {
                if idx > 0 {
                    spans.push(Span::styled(", ", Style::default().fg(app.theme.muted)));
                }
                spans.push(Span::styled(
                    flag.clone(),
                    Style::default().fg(app.theme.fg),
                ));
            }
            spans
        };
        rows.push(("Flags:", flag_spans));

        if let Some(d) = details {
            if let Some(cs) = d.cloud_saves {
                let cs_text = if cs { "Supported" } else { "Not supported" };
                rows.push((
                    "Cloud Saves:",
                    vec![Span::styled(
                        cs_text,
                        Style::default().fg(if cs { app.theme.green } else { app.theme.muted }),
                    )],
                ));
            }
            if !d.prerequisites.is_empty() {
                rows.push((
                    "Prereqs:",
                    vec![Span::styled(
                        d.prerequisites.join(", "),
                        Style::default().fg(app.theme.fg),
                    )],
                ));
            }
            if !d.installed_dlc.is_empty() {
                rows.push((
                    "DLCs (inst):",
                    vec![Span::styled(
                        d.installed_dlc.join(", "),
                        Style::default().fg(app.theme.fg),
                    )],
                ));
            }
        }
    } else {
        // Uninstalled game view
        let is_active_dl = app.active_install.as_ref().map(|i| &i.app_name) == Some(&g.app_name);
        let queue_pos = app.install_queue.iter().position(|q| q == &g.app_name);

        if is_active_dl {
            let inst = app.active_install.as_ref().unwrap();
            rows.push((
                "Status:",
                vec![Span::styled(
                    format!("Downloading ({:.1}%)", inst.progress.percentage),
                    Style::default()
                        .fg(app.theme.cyan)
                        .add_modifier(Modifier::BOLD),
                )],
            ));
            let (filled_bar, empty_bar) =
                format_download_progress_bar(inst.progress.percentage, 20);
            rows.push((
                "Progress:",
                vec![
                    Span::styled("[", Style::default().fg(app.theme.muted)),
                    Span::styled(filled_bar, Style::default().fg(app.theme.accent)),
                    Span::styled(empty_bar, Style::default().fg(app.theme.muted)),
                    Span::styled(
                        format!("] {:.1}%", inst.progress.percentage),
                        Style::default().fg(app.theme.fg),
                    ),
                ],
            ));
            rows.push((
                "Speed / ETA:",
                vec![Span::styled(
                    format!(
                        "{} • ETA: {}",
                        inst.progress.speed_str, inst.progress.eta_str
                    ),
                    Style::default().fg(app.theme.fg),
                )],
            ));
            if inst.progress.downloaded_bytes > 0 || inst.progress.total_bytes > 0 {
                let dl_bytes = prefix::fmt_size(inst.progress.downloaded_bytes);
                let total_bytes = if inst.progress.total_bytes > 0 {
                    prefix::fmt_size(inst.progress.total_bytes)
                } else {
                    "Unknown".to_string()
                };
                rows.push((
                    "Downloaded:",
                    vec![Span::styled(
                        format!("{dl_bytes} / {total_bytes}"),
                        Style::default().fg(app.theme.fg),
                    )],
                ));
            }
        } else if let Some(pos) = queue_pos {
            rows.push((
                "Status:",
                vec![Span::styled(
                    format!("Queued for download (position #{})", pos + 1),
                    Style::default()
                        .fg(app.theme.yellow)
                        .add_modifier(Modifier::BOLD),
                )],
            ));
        }

        let is_active_vfy = app.active_verify.as_ref().map(|v| &v.app_name) == Some(&g.app_name);

        if is_active_dl {
            // Downloading rows already pushed above
        } else if queue_pos.is_some() {
            // Queued rows already pushed above
        } else if is_active_vfy {
            let vfy = app.active_verify.as_ref().unwrap();
            rows.push((
                "Status:",
                vec![Span::styled(
                    format!("Verifying ({:.1}%)", vfy.progress.percentage),
                    Style::default()
                        .fg(app.theme.green)
                        .add_modifier(Modifier::BOLD),
                )],
            ));
            let (filled_bar, empty_bar) = format_download_progress_bar(vfy.progress.percentage, 20);
            rows.push((
                "Progress:",
                vec![
                    Span::styled("[", Style::default().fg(app.theme.muted)),
                    Span::styled(filled_bar, Style::default().fg(app.theme.green)),
                    Span::styled(empty_bar, Style::default().fg(app.theme.muted)),
                    Span::styled(
                        format!("] {:.1}%", vfy.progress.percentage),
                        Style::default().fg(app.theme.fg),
                    ),
                ],
            ));
            rows.push((
                "Speed / File:",
                vec![Span::styled(
                    format!(
                        "{} • Files: {}",
                        vfy.progress.speed_str,
                        vfy.progress.files_checked_str()
                    ),
                    Style::default().fg(app.theme.fg),
                )],
            ));
            if vfy.progress.bad_files > 0 {
                rows.push((
                    "Bad files:",
                    vec![Span::styled(
                        format!("{}", vfy.progress.bad_files),
                        Style::default()
                            .fg(app.theme.red)
                            .add_modifier(Modifier::BOLD),
                    )],
                ));
            }
        } else {
            show_install_hint = true;

            rows.push((
                "State:",
                vec![Span::styled(
                    "Not installed",
                    Style::default().fg(app.theme.muted),
                )],
            ));

            let ver = g
                .version
                .as_deref()
                .or_else(|| details.and_then(|d| d.version.as_deref()))
                .unwrap_or("-");
            if ver != "-" {
                rows.push((
                    "Version:",
                    vec![Span::styled(ver, Style::default().fg(app.theme.fg))],
                ));
            } else {
                rows.push((
                    "Version:",
                    vec![Span::styled("-", Style::default().fg(app.theme.muted))],
                ));
            }

            if let Some(dl) = details.and_then(|d| d.download_size) {
                let ext = details.and_then(|d| d.external_activation.as_deref());
                let third = details.and_then(|d| d.third_party_managed.as_deref());
                let dl_str = prefix::fmt_game_size(dl, ext, third);
                rows.push((
                    "Download:",
                    vec![Span::styled(dl_str, Style::default().fg(app.theme.yellow))],
                ));
            } else {
                rows.push((
                    "Download:",
                    vec![Span::styled("-", Style::default().fg(app.theme.muted))],
                ));
            }

            if let Some(inst_sz) = details.and_then(|d| d.installed_size) {
                let ext = details.and_then(|d| d.external_activation.as_deref());
                let third = details.and_then(|d| d.third_party_managed.as_deref());
                let inst_str = prefix::fmt_game_size(inst_sz, ext, third);
                rows.push((
                    "Install:",
                    vec![Span::styled(inst_str, Style::default().fg(app.theme.fg))],
                ));
            } else {
                rows.push((
                    "Install:",
                    vec![Span::styled("-", Style::default().fg(app.theme.muted))],
                ));
            }

            let free_space = crate::filesystem::available_disk_space(std::path::Path::new(
                &app.cfg.default_install_path,
            ));
            if let Some(free) = free_space {
                let free_str = prefix::fmt_size(free);
                let is_low = details
                    .and_then(|d| d.installed_size)
                    .map(|sz| sz > free)
                    .unwrap_or(false);
                let style = if is_low {
                    Style::default()
                        .fg(app.theme.red)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(app.theme.fg)
                };
                rows.push(("Free space:", vec![Span::styled(free_str, style)]));
            } else {
                rows.push((
                    "Free space:",
                    vec![Span::styled("-", Style::default().fg(app.theme.muted))],
                ));
            }

            let pdb_tier = details
                .and_then(|d| d.protondb_tier.as_deref())
                .or_else(|| app.metadata_mgr.get_protondb_tier(&g.app_name));
            let (_, pdb_val) = protondb_badge(pdb_tier, &app.theme);
            rows.push(("ProtonDB:", vec![pdb_val]));

            if let Some(d) = details {
                if let Some(cs) = d.cloud_saves {
                    let cs_text = if cs { "Supported" } else { "Not supported" };
                    rows.push((
                        "Cloud Saves:",
                        vec![Span::styled(
                            cs_text,
                            Style::default().fg(if cs { app.theme.green } else { app.theme.muted }),
                        )],
                    ));
                } else {
                    rows.push((
                        "Cloud Saves:",
                        vec![Span::styled("-", Style::default().fg(app.theme.muted))],
                    ));
                }

                if let Some(ref gd) = d.grant_date {
                    rows.push((
                        "Owned since:",
                        vec![Span::styled(gd.clone(), Style::default().fg(app.theme.fg))],
                    ));
                }

                if let Some(ref exe) = d.launch_exe {
                    rows.push((
                        "Executable:",
                        vec![Span::styled(exe.clone(), Style::default().fg(app.theme.fg))],
                    ));
                }

                if !d.prerequisites.is_empty() {
                    rows.push((
                        "Requires:",
                        vec![Span::styled(
                            d.prerequisites.join(", "),
                            Style::default().fg(app.theme.fg),
                        )],
                    ));
                }

                if !d.owned_dlc.is_empty() {
                    rows.push((
                        "DLC:",
                        vec![Span::styled(
                            format!("{} owned", d.owned_dlc.len()),
                            Style::default().fg(app.theme.fg),
                        )],
                    ));
                }
            } else {
                rows.push((
                    "Cloud Saves:",
                    vec![Span::styled("-", Style::default().fg(app.theme.muted))],
                ));
            }
        }
    }

    if let Some(d) = details {
        if !d.genres.is_empty() {
            let non_generic: Vec<&str> = d
                .genres
                .iter()
                .filter(|x| *x != "games" && *x != "applications" && *x != "public")
                .map(|s| s.as_str())
                .collect();
            if !non_generic.is_empty() {
                rows.push((
                    "Genre:",
                    vec![Span::styled(
                        non_generic.join(" / "),
                        Style::default().fg(app.theme.fg),
                    )],
                ));
            }
        }
    }

    // Format aligned label/value rows
    let max_label_len = rows
        .iter()
        .map(|(label, _)| label.width())
        .max()
        .unwrap_or(0);
    let label_w = max_label_len + 1;
    let value_w = (area.width as usize).saturating_sub(label_w);

    let desc_opt = details.and_then(|d| d.description.as_deref());
    let show_description = should_show_description(desc_opt, &g.title);

    if show_description {
        // Reserve: 1 blank line + 1 "Description" header + at least 6 description lines = 8 lines (10 if install hint)
        let reserve = if show_install_hint { 10 } else { 8 };
        let available_for_rows = (area.height as usize).saturating_sub(lines.len() + reserve);
        rows = filter_rows_by_priority(rows, available_for_rows);
    }

    for (label, value_spans) in rows {
        let is_path = matches!(label, "Prefix:" | "Executable:");
        let padded_label = format!("{:<width$}", label, width = label_w);

        if is_path {
            let path_str = value_spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>();
            let shortened = shorten_path(&path_str, value_w);
            let line_spans = vec![
                Span::styled(padded_label, Style::default().fg(app.theme.cyan)),
                Span::styled(shortened, Style::default().fg(app.theme.fg)),
            ];
            lines.push(Line::from(line_spans));
        } else if label == "Version:" {
            let ver_str = value_spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>();
            let truncated = truncate_with_ellipsis(&ver_str, value_w);
            let line_spans = vec![
                Span::styled(padded_label, Style::default().fg(app.theme.cyan)),
                Span::styled(truncated, Style::default().fg(app.theme.fg)),
            ];
            lines.push(Line::from(line_spans));
        } else {
            let total_val_width: usize = value_spans.iter().map(|s| s.content.width()).sum();
            if total_val_width <= value_w || value_spans.len() > 1 {
                let mut line_spans = vec![Span::styled(
                    padded_label,
                    Style::default().fg(app.theme.cyan),
                )];
                line_spans.extend(value_spans);
                lines.push(Line::from(line_spans));
            } else {
                let full_text = &value_spans[0].content;
                let text_style = value_spans[0].style;
                let words: Vec<&str> = full_text.split_whitespace().collect();
                let mut wrapped = Vec::new();
                let mut cur = String::new();
                for w in words {
                    if !cur.is_empty() && (cur.width() + 1 + w.width()) > value_w {
                        wrapped.push(cur);
                        cur = String::new();
                    }
                    if !cur.is_empty() {
                        cur.push(' ');
                    }
                    cur.push_str(w);
                }
                if !cur.is_empty() {
                    wrapped.push(cur);
                }
                if wrapped.is_empty() {
                    wrapped.push(String::new());
                }

                if wrapped.len() > 2 {
                    let mut second = wrapped[1].clone();
                    if second.width() + 3 > value_w {
                        let keep_w = value_w.saturating_sub(3);
                        let mut short = String::new();
                        for c in second.chars() {
                            let cw = UnicodeWidthChar::width(c).unwrap_or(1);
                            if short.width() + cw > keep_w {
                                break;
                            }
                            short.push(c);
                        }
                        second = format!("{}...", short);
                    } else {
                        second.push_str("...");
                    }
                    wrapped.truncate(2);
                    wrapped[1] = second;
                }

                lines.push(Line::from(vec![
                    Span::styled(padded_label, Style::default().fg(app.theme.cyan)),
                    Span::styled(wrapped[0].clone(), text_style),
                ]));

                if wrapped.len() > 1 {
                    lines.push(Line::from(vec![
                        Span::raw(" ".repeat(label_w)),
                        Span::styled(wrapped[1].clone(), text_style),
                    ]));
                }
            }
        }
    }

    if show_install_hint {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(
                "Enter  ",
                Style::default()
                    .fg(app.theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("install", Style::default().fg(app.theme.muted)),
        ]));
    }

    if show_description {
        // Blank line
        lines.push(Line::from(""));

        // 4. "Description" heading + word-wrapped description
        let used_rows = lines.len() as u16;
        let desc_max_w = (area.width.saturating_sub(1) as usize).max(20);
        let wrapped_lines = wrap_text(desc_opt.unwrap(), desc_max_w);

        let available_rows = area.height.saturating_sub(used_rows + 1);
        let total_desc_lines = wrapped_lines.len() as u16;
        let max_scroll = total_desc_lines.saturating_sub(available_rows);
        let scroll_y = app.detail_scroll.min(max_scroll);

        let desc_title = Span::styled(
            "Description",
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        );

        let heading_line = if max_scroll > 0 {
            let hint_str = format!("[{}/{}]", scroll_y + 1, max_scroll + 1);
            let hint_len = hint_str.chars().count();
            let pad = (area.width as usize).saturating_sub(11 + hint_len);
            Line::from(vec![
                desc_title,
                Span::raw(" ".repeat(pad)),
                Span::styled(hint_str, Style::default().fg(app.theme.muted)),
            ])
        } else {
            Line::from(vec![desc_title])
        };
        lines.push(heading_line);

        for l in wrapped_lines
            .into_iter()
            .skip(scroll_y as usize)
            .take(available_rows as usize)
        {
            lines.push(Line::from(Span::styled(
                l,
                Style::default().fg(app.theme.fg),
            )));
        }
    }

    f.render_widget(Paragraph::new(lines), area);
}

fn draw_cover(f: &mut Frame, app: &mut App, cover_area: Option<Rect>) {
    let Some(art_rect) = cover_area else {
        return;
    };
    if art_rect.width == 0 || art_rect.height == 0 {
        return;
    }

    match &app.cover_state {
        crate::artwork::CoverState::Ready => {
            let img_widget =
                ratatui_image::StatefulImage::default().resize(ratatui_image::Resize::Fit(None));

            // If wide image (aspect > 0.9), calculate centered sub-rect
            let render_rect = if let Some((w, h)) = app.current_image_size {
                let aspect = w as f32 / h as f32;
                if aspect > 0.9 {
                    let fs = app.picker.font_size();
                    let (cw, ch) = if fs.width > 0 && fs.height > 0 {
                        (fs.width as f32, fs.height as f32)
                    } else {
                        (10.0, 20.0)
                    };
                    let cell_aspect = cw / ch;
                    let target_height =
                        ((art_rect.width as f32 / aspect) * cell_aspect).round() as u16;
                    let target_h = target_height.clamp(1, art_rect.height);
                    let y_offset = (art_rect.height.saturating_sub(target_h)) / 2;
                    Rect::new(art_rect.x, art_rect.y + y_offset, art_rect.width, target_h)
                } else {
                    art_rect
                }
            } else {
                art_rect
            };

            f.render_stateful_widget(img_widget, render_rect, &mut app.thread_protocol);
        }
        crate::artwork::CoverState::Loading => {
            let content_lines = vec![
                Line::from(Span::styled(
                    "⏳ Loading...",
                    Style::default().fg(app.theme.cyan),
                )),
                Line::from(Span::styled(
                    "Fetching cover artwork…",
                    Style::default().fg(app.theme.muted),
                )),
            ];
            let pad_top = (art_rect.height.saturating_sub(content_lines.len() as u16)) / 2;
            let mut lines = Vec::new();
            for _ in 0..pad_top {
                lines.push(Line::from(""));
            }
            lines.extend(content_lines);
            let p = Paragraph::new(lines).alignment(Alignment::Center);
            f.render_widget(p, art_rect);
        }
        crate::artwork::CoverState::NoArtwork => {
            // Render handsome themed text card
            let card_w = (art_rect.width.saturating_sub(2) as usize).max(6);
            let title = app
                .current()
                .map(|g| g.title.as_str())
                .unwrap_or("No Artwork");
            let dev = app
                .current_details()
                .and_then(|d| d.developer.as_deref())
                .unwrap_or("");

            let mut card_lines = Vec::new();
            for l in wrap_text(title, card_w) {
                card_lines.push(Line::from(Span::styled(
                    l,
                    Style::default()
                        .fg(app.theme.accent)
                        .add_modifier(Modifier::BOLD),
                )));
            }
            if !dev.is_empty() {
                card_lines.push(Line::from(""));
                for l in wrap_text(dev, card_w) {
                    card_lines.push(Line::from(Span::styled(
                        l,
                        Style::default().fg(app.theme.muted),
                    )));
                }
            }
            card_lines.push(Line::from(""));
            card_lines.push(Line::from(Span::styled(
                "[R] Retry",
                Style::default().fg(app.theme.muted),
            )));

            let pad_top = (art_rect.height.saturating_sub(card_lines.len() as u16)) / 2;
            let mut lines = Vec::new();
            for _ in 0..pad_top {
                lines.push(Line::from(""));
            }
            lines.extend(card_lines);
            let p = Paragraph::new(lines).alignment(Alignment::Center);
            f.render_widget(p, art_rect);
        }
        crate::artwork::CoverState::Failed(reason) => {
            let content_lines = vec![
                Line::from(Span::styled(
                    "Artwork Unavailable",
                    Style::default().fg(app.theme.yellow),
                )),
                Line::from(Span::styled(
                    reason.clone(),
                    Style::default().fg(app.theme.muted),
                )),
                Line::from(Span::styled(
                    "[R] Retry",
                    Style::default().fg(app.theme.accent),
                )),
            ];
            let pad_top = (art_rect.height.saturating_sub(content_lines.len() as u16)) / 2;
            let mut lines = Vec::new();
            for _ in 0..pad_top {
                lines.push(Line::from(""));
            }
            lines.extend(content_lines);
            let p = Paragraph::new(lines).alignment(Alignment::Center);
            f.render_widget(p, art_rect);
        }
        crate::artwork::CoverState::Idle => {
            let content_lines = vec![Line::from(Span::styled(
                "No Artwork",
                Style::default().fg(app.theme.muted),
            ))];
            let pad_top = (art_rect.height.saturating_sub(content_lines.len() as u16)) / 2;
            let mut lines = Vec::new();
            for _ in 0..pad_top {
                lines.push(Line::from(""));
            }
            lines.extend(content_lines);
            let p = Paragraph::new(lines).alignment(Alignment::Center);
            f.render_widget(p, art_rect);
        }
    }
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
                    .fg(app.theme.fg)
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
    let bottom_hint = match app.mode() {
        AppMode::Menu(m) if matches!(m.kind, crate::app::MenuKind::CloudSaves(_)) => {
            " [l] List  [s] Sync  [d] Download  [Enter] Select  [Esc] Back "
        }
        _ => " [h/l · ←/→] Adjust  [Enter] Select  [Esc] Back ",
    };

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
            bottom_hint,
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
        .map(|l| {
            if l.starts_with("WARNING:") {
                Line::from(Span::styled(
                    l.clone(),
                    Style::default()
                        .fg(app.theme.red)
                        .add_modifier(Modifier::BOLD),
                ))
            } else {
                Line::from(Span::styled(l.clone(), Style::default().fg(app.theme.fg)))
            }
        })
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
    let r = if area.width < 70 {
        area
    } else {
        let h = (rows.len() as u16 + 5).min(area.height - 4).max(8);
        centered(area, 72.min(area.width - 4), h)
    };
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
    let r = centered(area, 72.min(area.width - 4), 32.min(area.height - 2));
    f.render_widget(Clear, r);
    let mut text = vec![
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
            Span::styled("c                 ", Style::default().fg(app.theme.cyan)),
            Span::raw("Cloud Saves (list, sync, download)"),
        ]),
        Line::from(vec![
            Span::styled("v                 ", Style::default().fg(app.theme.cyan)),
            Span::raw("Verify game files against manifest"),
        ]),
        Line::from(vec![
            Span::styled("i                 ", Style::default().fg(app.theme.cyan)),
            Span::raw("Import existing game installation"),
        ]),
        Line::from(vec![
            Span::styled("u                 ", Style::default().fg(app.theme.cyan)),
            Span::raw("Check for updates / update selected"),
        ]),
        Line::from(vec![
            Span::styled("d                 ", Style::default().fg(app.theme.cyan)),
            Span::raw("Delete game menu (installed)"),
        ]),
        Line::from(vec![
            Span::styled("x                 ", Style::default().fg(app.theme.cyan)),
            Span::raw("Cancel active download, queue, or verification"),
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
            Span::styled("R                 ", Style::default().fg(app.theme.cyan)),
            Span::raw("Refresh / re-download cover artwork"),
        ]),
        Line::from(vec![
            Span::styled("s                 ", Style::default().fg(app.theme.cyan)),
            Span::raw("Settings (Cleanup, Account, Proton, LSFG)"),
        ]),
        Line::from(vec![
            Span::styled("o                 ", Style::default().fg(app.theme.cyan)),
            Span::raw("Open Epic Games Store page in browser"),
        ]),
        Line::from(vec![
            Span::styled("?                 ", Style::default().fg(app.theme.cyan)),
            Span::raw("Show this help dialog"),
        ]),
        Line::from(vec![
            Span::styled("q / Esc           ", Style::default().fg(app.theme.cyan)),
            Span::raw("Back / Quit"),
        ]),
    ];

    if let Some(g) = app.current() {
        text.push(Line::from(""));
        text.push(Line::from(vec![
            Span::styled(
                "Selected Game:    ",
                Style::default()
                    .fg(app.theme.accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(g.title.clone(), Style::default().fg(app.theme.fg)),
        ]));
        if let Some(d) = app.current_details() {
            if let Some(folder) = &d.cloud_save_folder {
                text.push(Line::from(vec![
                    Span::styled("Cloud Save Path:  ", Style::default().fg(app.theme.cyan)),
                    Span::styled(folder.clone(), Style::default().fg(app.theme.fg)),
                ]));
            }
        }
    }

    text.push(Line::from(""));
    text.push(Line::from(Span::styled(
        "Dedicated Epic Prefixes: ~/.local/share/legendary-tui/prefixes/<appid>/",
        Style::default().fg(app.theme.muted),
    )));
    text.push(Line::from(Span::styled(
        "Alt+G Menu Integration: ~/.config/rofi/epic-games.list",
        Style::default().fg(app.theme.muted),
    )));
    f.render_widget(
        Paragraph::new(text).block(solid_block("Keyboard Navigation & Controls", &app.theme)),
        r,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_layout_rect_computation_breakpoints() {
        // Full breakpoint (>= 100)
        let full_area = Rect::new(0, 0, 120, 40);
        let layout_full = compute_layout(full_area, false, (10, 20));
        assert_eq!(layout_full.breakpoint, Breakpoint::Full);
        assert!(layout_full.divider.is_some());
        assert!(layout_full.detail.is_some());
        assert!(layout_full.cover.is_some());
        assert!(layout_full.info.is_some());
        assert!(layout_full.cover_source.is_none());
        assert_eq!(layout_full.footer.height, 2);

        // Medium breakpoint (70 <= w < 100)
        let med_area = Rect::new(0, 0, 85, 40);
        let layout_med = compute_layout(med_area, false, (10, 20));
        assert_eq!(layout_med.breakpoint, Breakpoint::Medium);
        assert!(layout_med.divider.is_some());
        assert!(layout_med.detail.is_some());
        assert!(layout_med.cover.is_none());
        assert!(layout_med.info.is_some());
        assert!(layout_med.cover_source.is_none());

        // Compact breakpoint (< 70)
        let compact_area = Rect::new(0, 0, 65, 40);
        let layout_compact = compute_layout(compact_area, false, (10, 20));
        assert_eq!(layout_compact.breakpoint, Breakpoint::Compact);
        assert!(layout_compact.divider.is_none());
        assert!(layout_compact.detail.is_none());
        assert_eq!(layout_compact.list, layout_compact.body);

        // Footer is 2 lines permanently when height >= 6; list height identical regardless of status/search (no jump)
        let layout_status = compute_layout(full_area, true, (10, 20));
        assert_eq!(layout_status.footer.height, 2);
        assert_eq!(layout_status.list.height, layout_full.list.height);
    }

    #[test]
    fn test_cover_rect_keeps_2_3_ratio_various_cell_sizes() {
        let container = Rect::new(40, 1, 80, 30);
        for (cw, ch) in [(14, 37), (8, 16), (10, 20)] {
            let cover = compute_cover_rect(container, (cw, ch));
            assert!(cover.width <= 40);
            assert!(cover.height < container.height);
            assert!(cover.width > 0 && cover.height > 0);
            let pixel_w = cover.width as f64 * cw as f64;
            let pixel_h = cover.height as f64 * ch as f64;
            let aspect = pixel_w / pixel_h;
            assert!(
                (aspect - (2.0 / 3.0)).abs() < 0.05,
                "Aspect ratio {aspect} for font size ({cw}, {ch}) deviated from 2/3"
            );
        }
    }

    #[test]
    fn test_info_column_description_clearance() {
        let facts_rows = 12;

        let area_120 = Rect::new(0, 0, 120, 30);
        let layout_120 = compute_layout(area_120, false, (10, 20));
        let info_120 = layout_120
            .info
            .expect("info column should exist at 120 cols");
        assert_eq!(info_120.height, layout_120.body.height);
        let desc_rows_120 = info_120.height.saturating_sub(facts_rows);
        assert!(
            desc_rows_120 >= 8,
            "Expected at least 8 description rows at 120x30, got {desc_rows_120}"
        );

        let area_100 = Rect::new(0, 0, 100, 24);
        let layout_100 = compute_layout(area_100, false, (10, 20));
        let info_100 = layout_100
            .info
            .expect("info column should exist at 100 cols");
        let desc_rows_100 = info_100.height.saturating_sub(facts_rows);
        assert!(
            desc_rows_100 >= 8,
            "Expected at least 8 description rows at 100x24, got {desc_rows_100}"
        );
    }

    #[test]
    fn test_medium_breakpoint_full_info_width() {
        let area = Rect::new(0, 0, 85, 40);
        let layout = compute_layout(area, false, (10, 20));
        assert_eq!(layout.breakpoint, Breakpoint::Medium);
        assert!(layout.cover.is_none());
        let info = layout
            .info
            .expect("info column should exist in medium breakpoint");
        let detail = layout
            .detail
            .expect("detail area should exist in medium breakpoint");
        assert_eq!(info.width, detail.width);
        assert_eq!(info.x, detail.x);
    }

    #[test]
    fn test_label_column_alignment() {
        let labels = ["State:", "Version:", "Cloud Saves:", "ProtonDB:"];
        let max_len = labels.iter().map(|l| l.chars().count()).max().unwrap();
        assert_eq!(max_len, 12);
        let label_w = max_len + 1;
        assert_eq!(label_w, 13);
        for l in labels {
            let formatted = format!("{:<width$}", l, width = label_w);
            assert_eq!(formatted.len(), 13);
            assert!(formatted.starts_with(l));
        }
    }

    #[test]
    fn test_no_source_label_in_main_view() {
        for w in [120, 85, 65] {
            let area = Rect::new(0, 0, w, 30);
            let layout = compute_layout(area, false, (10, 20));
            assert!(
                layout.cover_source.is_none(),
                "cover_source must be None in main UI for width {w}"
            );
        }
    }

    #[test]
    fn test_footer_hint_set() {
        let normal_hints = footer_hints(false, false);
        assert!(normal_hints.contains("j/k move"));
        assert!(normal_hints.contains("Enter install"));
        assert!(normal_hints.contains("/ search"));
        assert!(normal_hints.contains("f filter"));
        assert!(normal_hints.contains("r refresh"));
        assert!(normal_hints.contains("? help"));
        assert!(normal_hints.contains("q quit"));

        let installed_hints = footer_hints(true, false);
        assert!(installed_hints.contains("Enter options"));

        let job_hints = footer_hints(false, true);
        assert!(job_hints.contains("x cancel"));
    }

    #[test]
    fn test_format_footer_spans_styling() {
        let theme = crate::theme::load();
        let hints = "j/k move   Enter install   ? help";
        let spans = format_footer_spans(hints, &theme);
        assert!(!spans.is_empty());
        let key_span = spans
            .iter()
            .find(|s| s.content == "j/k")
            .expect("j/k key span exists");
        assert!(key_span.style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn test_checkbox_column_visibility() {
        assert!(!should_show_checkboxes(0));
        assert!(should_show_checkboxes(1));
        assert!(should_show_checkboxes(42));
    }

    #[test]
    fn test_shorten_path_behavior() {
        // Under width
        assert_eq!(shorten_path("/opt/games/bin", 20), "/opt/games/bin");

        // Long path gets shortened and never exceeds max_width
        let long_path =
            "/home/user/.local/share/legendary-tui/prefixes/c22fcf8eec61424b9901f2888843a25a";
        let shortened = shorten_path(long_path, 35);
        assert!(shortened.width() <= 35);
        assert!(shortened.contains("..."));

        // Very small max_width
        assert_eq!(shorten_path("/some/path", 3), "...");
        assert_eq!(shorten_path("/some/path", 2), "..");
    }

    #[test]
    fn test_format_enabled_flags() {
        assert_eq!(format_enabled_flags(false, false, "Disabled"), vec!["None"]);
        assert_eq!(
            format_enabled_flags(true, false, "Disabled"),
            vec!["GameMode"]
        );
        assert_eq!(
            format_enabled_flags(true, true, "x2"),
            vec!["GameMode", "MangoHud", "LSFG (x2)"]
        );
    }

    #[test]
    fn test_should_show_description() {
        assert!(!should_show_description(None, "Warhammer 40K"));
        assert!(!should_show_description(Some(""), "Warhammer 40K"));
        assert!(!should_show_description(Some("   "), "Warhammer 40K"));
        assert!(!should_show_description(
            Some("Warhammer 40K"),
            "Warhammer 40K"
        ));
        assert!(!should_show_description(
            Some("warhammer 40k"),
            "Warhammer 40K"
        ));
        assert!(should_show_description(
            Some("A combat racing game set in the Warhammer 40,000 universe."),
            "Warhammer 40K"
        ));
    }

    #[test]
    fn test_filter_rows_by_priority() {
        let initial_rows: Vec<(&'static str, Vec<Span<'static>>)> = vec![
            ("State:", vec![Span::raw("Installed")]),
            ("Version:", vec![Span::raw("1.0")]),
            ("Executable:", vec![Span::raw("game.exe")]),
            ("Prefix:", vec![Span::raw("~/.prefix")]),
            ("Flags:", vec![Span::raw("GameMode")]),
            ("Prereqs:", vec![Span::raw("DirectX")]),
        ];

        // If 6 rows available, keep all 6
        let res6 = filter_rows_by_priority(initial_rows.clone(), 6);
        assert_eq!(res6.len(), 6);

        // If 5 rows available, drop Flags:
        let res5 = filter_rows_by_priority(initial_rows.clone(), 5);
        assert_eq!(res5.len(), 5);
        assert!(!res5.iter().any(|(l, _)| *l == "Flags:"));

        // If 4 rows available, drop Flags: and Prefix:
        let res4 = filter_rows_by_priority(initial_rows.clone(), 4);
        assert_eq!(res4.len(), 4);
        assert!(!res4.iter().any(|(l, _)| *l == "Flags:" || *l == "Prefix:"));

        // If 3 rows available, drop Flags:, Prefix:, and Executable:
        let res3 = filter_rows_by_priority(initial_rows.clone(), 3);
        assert_eq!(res3.len(), 3);
        assert!(!res3
            .iter()
            .any(|(l, _)| *l == "Flags:" || *l == "Prefix:" || *l == "Executable:"));

        // If 2 rows available, drop Flags:, Prefix:, Executable:, and Prereqs:
        let res2 = filter_rows_by_priority(initial_rows.clone(), 2);
        assert_eq!(res2.len(), 2);
        assert_eq!(res2[0].0, "State:");
        assert_eq!(res2[1].0, "Version:");
    }

    #[test]
    fn test_top_title_excludes_search_query() {
        let app = crate::app::tests::test_app();
        let count_text = format!("{}", app.games.len());
        let title_str = format!(" Library [{}] ({count_text}) ", app.filter.label());
        assert!(!title_str.contains("• /"));
        assert!(title_str.contains("Library"));
    }

    #[test]
    fn test_filter_rows_by_priority_uninstalled() {
        let uninstalled_rows = vec![
            ("State:", vec![Span::raw("Uninstalled")]),
            ("Version:", vec![Span::raw("1.0")]),
            ("Download:", vec![Span::raw("10 GB")]),
            ("Install:", vec![Span::raw("12 GB")]),
            ("Free space:", vec![Span::raw("50 GB")]),
            ("ProtonDB:", vec![Span::raw("Gold")]),
            ("Cloud Saves:", vec![Span::raw("Supported")]),
            ("Owned since:", vec![Span::raw("2023-01-01")]),
            ("Executable:", vec![Span::raw("game.exe")]),
            ("Requires:", vec![Span::raw("Visual C++")]),
            ("DLC:", vec![Span::raw("2 owned")]),
        ];

        // 11 rows available -> keep all 11
        let res11 = filter_rows_by_priority(uninstalled_rows.clone(), 11);
        assert_eq!(res11.len(), 11);

        // 10 rows -> drop Requires:
        let res10 = filter_rows_by_priority(uninstalled_rows.clone(), 10);
        assert_eq!(res10.len(), 10);
        assert!(!res10.iter().any(|(l, _)| *l == "Requires:"));

        // 9 rows -> drop Requires: and Executable:
        let res9 = filter_rows_by_priority(uninstalled_rows.clone(), 9);
        assert_eq!(res9.len(), 9);
        assert!(!res9
            .iter()
            .any(|(l, _)| *l == "Requires:" || *l == "Executable:"));

        // 8 rows -> drop Requires:, Executable:, Owned since:
        let res8 = filter_rows_by_priority(uninstalled_rows.clone(), 8);
        assert_eq!(res8.len(), 8);
        assert!(!res8
            .iter()
            .any(|(l, _)| *l == "Requires:" || *l == "Executable:" || *l == "Owned since:"));

        // 7 rows -> drop Requires:, Executable:, Owned since:, DLC:
        let res7 = filter_rows_by_priority(uninstalled_rows.clone(), 7);
        assert_eq!(res7.len(), 7);
        assert!(!res7.iter().any(|(l, _)| *l == "Requires:"
            || *l == "Executable:"
            || *l == "Owned since:"
            || *l == "DLC:"));

        // 6 rows -> drop Requires:, Executable:, Owned since:, DLC:, Free space:
        let res6 = filter_rows_by_priority(uninstalled_rows.clone(), 6);
        assert_eq!(res6.len(), 6);
        assert!(!res6.iter().any(|(l, _)| *l == "Free space:"));
    }

    #[test]
    fn test_build_responsive_footer_spans_drop_order() {
        let theme = crate::theme::load();

        // Level 0 (>= 86 cols): has all hints
        let wide = build_responsive_footer_spans(false, None, 150, &theme);
        let wide_text: String = wide.iter().map(|s| s.content.as_ref()).collect();
        assert!(wide_text.contains("refresh"));
        assert!(wide_text.contains("filter"));
        assert!(wide_text.contains("epic"));
        assert!(wide_text.contains("search"));
        assert!(wide_text.contains("install"));

        // Level 1 (74..85 cols): drops refresh, keeps filter
        let l1 = build_responsive_footer_spans(false, None, 75, &theme);
        let l1_text: String = l1.iter().map(|s| s.content.as_ref()).collect();
        assert!(!l1_text.contains("refresh"));
        assert!(l1_text.contains("filter"));
        assert!(l1_text.contains("epic"));

        // Level 2 (63..73 cols): drops filter and refresh
        let l2 = build_responsive_footer_spans(false, None, 65, &theme);
        let l2_text: String = l2.iter().map(|s| s.content.as_ref()).collect();
        assert!(!l2_text.contains("refresh"));
        assert!(!l2_text.contains("filter"));
        assert!(l2_text.contains("epic"));

        // Level 3 (54..62 cols): drops epic
        let l3 = build_responsive_footer_spans(false, None, 55, &theme);
        let l3_text: String = l3.iter().map(|s| s.content.as_ref()).collect();
        assert!(!l3_text.contains("refresh"));
        assert!(!l3_text.contains("filter"));
        assert!(!l3_text.contains("epic"));
        assert!(l3_text.contains("search"));

        // Level 4 (core only): drops search
        let narrow = build_responsive_footer_spans(false, None, 45, &theme);
        let narrow_text: String = narrow.iter().map(|s| s.content.as_ref()).collect();
        assert!(!narrow_text.contains("refresh"));
        assert!(!narrow_text.contains("filter"));
        assert!(!narrow_text.contains("epic"));
        assert!(!narrow_text.contains("search"));
        assert!(narrow_text.contains("move"));
        assert!(narrow_text.contains("install"));
        assert!(narrow_text.contains("help"));
        assert!(narrow_text.contains("quit"));
    }

    #[test]
    fn test_footer_fit_at_140_100_70() {
        let theme = crate::theme::load();
        for &w in &[140, 100, 70] {
            let spans =
                build_responsive_footer_spans(false, Some("cancel Tomb Raider..."), w, &theme);
            let total_len: usize = spans.iter().map(|s| s.content.width()).sum();
            assert!(
                total_len <= w,
                "Footer spans total length {total_len} must fit within width {w}"
            );
            let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
            assert!(text.contains("cancel Tomb Raider..."));
            assert!(!text.contains('\n'));
        }
    }

    #[test]
    fn test_cancel_hint_labeling() {
        // When selected game is downloading: labeled "cancel"
        let label_selected = compute_cancel_label(
            Some((
                "tomb_raider",
                "Shadow of the Tomb Raider: Definitive Edition",
            )),
            None,
            Some("tomb_raider"),
        );
        assert_eq!(label_selected, Some("cancel".to_string()));

        // When another game is selected: labeled "cancel <title>" (truncated with ellipsis)
        let label_other = compute_cancel_label(
            Some((
                "tomb_raider",
                "Shadow of the Tomb Raider: Definitive Edition",
            )),
            None,
            Some("dishonored"),
        );
        assert_eq!(label_other, Some("cancel Shadow of the…".to_string()));

        // When no download or verify: None
        let label_none = compute_cancel_label(None, None, Some("dishonored"));
        assert_eq!(label_none, None);
    }

    #[test]
    fn test_progress_bar_chars() {
        let (filled, empty) = format_download_progress_bar(76.0, 20);
        assert!(!filled.is_empty());
        assert!(!empty.is_empty());
        assert!(filled.contains('━'));
        assert!(empty.contains('─'));
        assert!(!filled.contains('█'));
        assert!(!empty.contains('░'));
        assert_eq!(filled.chars().count() + empty.chars().count(), 20);
    }

    #[test]
    fn test_selected_is_downloading_shows_block_and_no_bar() {
        // Selected game is the downloading one
        let bar_line = compute_download_bar_line(true, "Dishonored", 45.0, "12 MB/s", "2m 10s", 0);
        assert!(
            bar_line.is_none(),
            "Bottom bar must be hidden when selected game is downloading"
        );
    }

    #[test]
    fn test_other_selected_shows_bar_and_no_block() {
        // Different game selected while download runs
        let bar_line =
            compute_download_bar_line(false, "Dishonored Definitive", 45.0, "12 MB/s", "2m 10s", 2);
        assert!(bar_line.is_some());
        let line = bar_line.unwrap();
        assert!(line.contains('↓'));
        assert!(line.contains("45.0%"));
        assert!(line.contains("12 MB/s"));
        assert!(line.contains("ETA 2m 10s"));
        assert!(line.contains("(+2 queued)"));
    }

    #[test]
    fn test_list_height_identical_in_both() {
        let area = Rect::new(0, 0, 120, 40);
        let layout_downloading_selected = compute_layout(area, false, (10, 20));
        let layout_other_selected_with_bar = compute_layout(area, true, (10, 20));
        assert_eq!(
            layout_downloading_selected.list.height, layout_other_selected_with_bar.list.height,
            "List height must be identical regardless of whether bottom bar is shown"
        );
        assert_eq!(layout_downloading_selected.footer.height, 2);
        assert_eq!(layout_other_selected_with_bar.footer.height, 2);
    }

    #[test]
    fn test_stage_and_actions_rows_absent() {
        let mut app = crate::app::tests::test_app();
        let uninstalled_pos = app
            .filtered_rows()
            .iter()
            .position(|r| match r {
                crate::app::RowItem::Game(idx) => !app.games[*idx].installed,
                _ => false,
            })
            .expect("must have an uninstalled game");
        app.selected = uninstalled_pos;
        let app_name = app.current().unwrap().app_name.clone();
        let title = app.current().unwrap().title.clone();
        app.active_install = Some(crate::install::ActiveInstall::mock_for_test(
            &app_name, &title,
        ));
        let backend = ratatui::backend::TestBackend::new(120, 40);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                draw_info(f, &app, Rect::new(40, 1, 78, 36));
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let mut full_screen = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                full_screen.push_str(buffer.cell((x, y)).unwrap().symbol());
            }
            full_screen.push('\n');
        }
        assert!(
            !full_screen.contains("Stage:"),
            "Stage row must not appear in detail pane"
        );
        assert!(
            !full_screen.contains("Cancel Download"),
            "Cancel Download action row must not appear in detail pane"
        );
        assert!(full_screen.contains("Status:"), "Status row must appear");
        assert!(
            full_screen.contains("Progress:"),
            "Progress row must appear"
        );
    }

    #[test]
    fn test_title_wrapping_at_100_and_120() {
        let long_title =
            "The Elder Scrolls V: Skyrim Special Edition - Very Long Subtitle That Exceeds Width";

        let wrapped_120 = wrap_title(long_title, 40);
        assert!(
            wrapped_120.len() <= 2,
            "Must not exceed 2 lines at width 40"
        );
        if wrapped_120.len() == 2 && wrapped_120[1].chars().count() >= 37 {
            assert!(wrapped_120[1].ends_with('…'));
        }

        let wrapped_100 = wrap_title(long_title, 30);
        assert!(
            wrapped_100.len() <= 2,
            "Must not exceed 2 lines at width 30"
        );
        assert!(wrapped_100[1].ends_with('…'));
    }
}

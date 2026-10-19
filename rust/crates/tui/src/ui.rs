//! Clavus TUI rendering — "Arch Dark" theme.

use crate::theme::*;
use crate::app::{App, Screen};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Table, Row},
    Frame,
};

// ─── Entry point ─────────────────────────────────────────────────────

/// Render the main frame.
pub fn render(frame: &mut Frame, app: &mut App) {
    match app.screen {
        Screen::ProjectList  => render_project_list(frame, app),
        Screen::ProjectDetail=> render_project_detail(frame, app),
        Screen::CueList      => render_cue_list(frame, app),
        Screen::CueAdd       => render_cue_add(frame, app),
        Screen::SnapshotList  => render_snapshot_list(frame, app),
        Screen::Sync         => render_sync(frame, app),
    }
}

// ─── Project List ────────────────────────────────────────────────────

fn render_project_list(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0), Constraint::Length(1)])
        .split(frame.size());
    let header = chunks[0];
    let body = chunks[1];
    let footer = chunks[2];

    render_title_bar(frame, header, "projects", app);
    render_footer(frame, footer, app);

    // ── Project table ──────────────────────────────────────────────
    // Columns: TYPE · NAME · BPM · TRACKS · MODIFIED
    let header_style = Style::new()
        .fg(MUTED)
        .bg(SURFACE)
        .add_modifier(Modifier::BOLD);

    let header_row = Row::new(vec![
        Span::raw("TYPE"),
        Span::raw("NAME"),
        Span::raw("BPM"),
        Span::raw("TRACKS"),
        Span::raw("MODIFIED"),
    ])
    .style(header_style)
    .height(1);

    let total = app.projects.len();
    let rows: Vec<Row<'_>> = app.projects
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let selected = Some(i) == app.selected_project;
            let bg = if selected { ACCENT } else { SURFACE };
            let fg = if selected { BG } else { TEXT };

            let modified = unix_to_relative(p.modified_secs);

            Row::new(vec![
                // Type badge
                Span::raw(if p.project_type == "Ableton" { "ALS" } else { "BW" }),
                // Name
                Span::raw(&p.name),
                // BPM
                Span::raw(format!("{:.0}", p.bpm)),
                // Track count
                Span::raw(p.track_count.to_string()),
                // Modified
                Span::raw(modified),
            ])
            .style(Style::new().fg(fg).bg(bg))
            .height(1)
        })
        .collect();

    let table = Table::new(rows, &[
        Constraint::Length(5),   // TYPE
        Constraint::Min(20),    // NAME
        Constraint::Length(6),  // BPM
        Constraint::Length(7),  // TRACKS
        Constraint::Length(10), // MODIFIED
    ])
    .header(header_row)
    .block(Block::default().borders(Borders::ALL).border_style(Style::new().fg(BORDER)))
    .style(S_TEXT);

    frame.render_widget(table, body);

    // Selected item highlight bar (thin line below selected row)
    if let Some(idx) = app.selected_project {
        let _table_height = body.height.saturating_sub(2) as usize; // subtract block borders
        if total > 0 && idx < total {
            let row_height = 1;
            let y = body.y + 1 + (idx as u16) * row_height;
            if y < body.y + body.height - 1 {
                let highlight = Paragraph::new("")
                    .style(Style::new().bg(ACCENT).fg(BG));
                frame.render_widget(highlight, Rect::new(body.x + 1, y, body.width - 2, 1));
            }
        }
    }
}

// ─── Project Detail ─────────────────────────────────────────────────

fn render_project_detail(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0), Constraint::Length(1)])
        .split(frame.size());
    let header = chunks[0];
    let body = chunks[1];
    let footer = chunks[2];

    let project = match &app.project_detail {
        Some(p) => p,
        None => {
            render_centered_error(frame, body, "No project loaded");
            return;
        }
    };

    let title = format!("{}  ·  {:.0} BPM  ·  {}", project.name, project.bpm, project.time_signature);
    render_title_bar(frame, header, &title, app);
    render_footer(frame, footer, app);

    // ── Two-column body ────────────────────────────────────────────
    let [tracks_col, info_col] = {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(body);
        [chunks[0], chunks[1]]
    };

    // Tracks table
    let header_style = Style::new().fg(MUTED).bg(SURFACE).add_modifier(Modifier::BOLD);
    let track_header = Row::new(vec![
        Span::raw("#"),
        Span::raw("NAME"),
        Span::raw("TYPE"),
    ])
    .style(header_style)
    .height(1);

    let track_rows: Vec<Row<'_>> = project.tracks.iter().enumerate().map(|(i, t)| {
        let icon = match t.track_type.as_str() {
            "Audio" => "🔊",
            "MIDI"  => "🎹",
            "Group" => "📂",
            _       => "•",
        };
        let is_audio = t.track_type == "Audio";
        Row::new(vec![
            Span::raw(format!("{:02}", i + 1)),
            Span::raw(format!("{} {}", icon, t.name)),
            Span::raw(&t.track_type),
        ])
        .style(Style::new().fg(if is_audio { TEXT } else { MUTED }).bg(SURFACE))
        .height(1)
    }).collect();

    let tracks_table = Table::new(track_rows, &[
        Constraint::Length(3),
        Constraint::Min(0),
        Constraint::Length(10),
    ])
    .header(track_header)
    .block(Block::default()
        .title(format!(" tracks  {} ", project.tracks.len()))
        .title_style(Style::new().fg(MUTED).add_modifier(Modifier::BOLD))
        .borders(Borders::ALL)
        .border_style(Style::new().fg(BORDER)))
    .style(S_TEXT);

    frame.render_widget(tracks_table, tracks_col);

    // Info + Markers panel
    let [info_area, marker_area] = {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Min(0)])
            .split(info_col);
        [chunks[0], chunks[1]]
    };

    // Project info
    let info_lines = vec![
        l("", MUTED),
        line("  TYPE      ", MUTED, project.project_type.to_string().as_str(), TEXT),
        line("  BPM       ", MUTED, format!("{:.1}", project.bpm), TEXT),
        line("  SIGNATURE ", MUTED, project.time_signature.clone(), TEXT),
        line("  TRACKS    ", MUTED, project.track_count.to_string(), TEXT),
        line("  MARKERS   ", MUTED, project.markers.len().to_string(), TEXT),
        if let Some(v) = &project.ableton_version {
            line("  VERSION   ", MUTED, v, MUTED)
        } else {
            l("", MUTED)
        },
    ];

    let info_para = Paragraph::new(info_lines)
        .block(Block::default()
            .title(" info ")
            .title_style(Style::new().fg(MUTED).add_modifier(Modifier::BOLD))
            .borders(Borders::ALL)
            .border_style(Style::new().fg(BORDER)))
        .style(S_BG);

    frame.render_widget(info_para, info_area);

    // Markers
    if project.markers.is_empty() {
        let empty = Paragraph::new(vec![l("", MUTED), l("  no markers", MUTED)])
            .block(Block::default()
                .title(" markers ")
                .title_style(Style::new().fg(MUTED).add_modifier(Modifier::BOLD))
                .borders(Borders::ALL)
                .border_style(Style::new().fg(BORDER_DIM)))
            .style(S_BG);
        frame.render_widget(empty, marker_area);
    } else {
        let marker_lines: Vec<Line<'_>> = project.markers.iter().map(|m| {
            Line::raw(format!("  {:>8}  {}", m.time, m.name))
                .style(Style::new().fg(GOLD))
        }).collect();
        let marker_para = Paragraph::new(marker_lines)
            .block(Block::default()
                .title(format!(" markers  {} ", project.markers.len()))
                .title_style(Style::new().fg(MUTED).add_modifier(Modifier::BOLD))
                .borders(Borders::ALL)
                .border_style(Style::new().fg(BORDER)))
            .style(S_BG);
        frame.render_widget(marker_para, marker_area);
    }
}

// ─── Cue List ───────────────────────────────────────────────────────

fn render_cue_list(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0), Constraint::Length(1)])
        .split(frame.size());
    let header = chunks[0];
    let body = chunks[1];
    let footer = chunks[2];

    let title = format!(" cues  ·  {} ", app.cues.len());
    render_title_bar(frame, header, &title, app);
    render_footer(frame, footer, app);

    let filter = app.cue_filter.clone();
    let filtered: Vec<_> = app.cues.iter()
        .filter(|c| filter.is_none() || filter.as_ref().is_some_and(|f| c.status == *f))
        .collect();

    if filtered.is_empty() {
        render_centered(frame, body, &[("no cues", MUTED)]);
        return;
    }

    // Filter indicator
    if let Some(f) = &filter {
        let filter_line = Paragraph::new(vec![Line::raw(format!("  filtered by: {:?}", f))])
            .style(Style::new().fg(MUTED).bg(SURFACE));
        frame.render_widget(filter_line, body);
    }

    let cue_rows: Vec<ListItem<'_>> = filtered.iter().map(|c| {
        let color = cue_status_color(&c.status);
        let icon  = cue_status_icon(&c.status);

        let line1 = Line::raw(format!(
            "{} {}  {:12}  {}",
            icon,
            c.status.as_str(),
            c.position,
            c.text,
        )).style(Style::new().fg(color));

        let line2 = Line::raw(format!(
            "      {} · {} · {}",
            c.track_name,
            &c.id[..8],
            c.author,
        )).style(Style::new().fg(MUTED));

        ListItem::new(vec![line1, line2])
    }).collect();

    let list = List::new(cue_rows)
        .block(Block::default()
            .borders(Borders::ALL)
            .border_style(Style::new().fg(BORDER)))
        .style(S_BG);

    frame.render_widget(list, body);
}

// ─── Cue Add ─────────────────────────────────────────────────────────

fn render_cue_add(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0), Constraint::Length(1)])
        .split(frame.size());
    let header = chunks[0];
    let body = chunks[1];
    let footer = chunks[2];

    render_title_bar(frame, header, "new cue", app);
    render_footer(frame, footer, app);

    let form = vec![
        l("", MUTED),
        l("  position (beats)", MUTED),
        line("  ", ACCENT, &app.input_buf, TEXT),
        l("", MUTED),
        l("  text", MUTED),
        line("  ", TEXT, "type here", MUTED),
        l("", MUTED),
        l("  track", MUTED),
        l("  Arrangement", TEXT),
    ];

    let form_para = Paragraph::new(form)
        .block(Block::default()
            .title(" add cue ")
            .title_style(Style::new().fg(ACCENT).add_modifier(Modifier::BOLD))
            .borders(Borders::ALL)
            .border_style(Style::new().fg(BORDER)))
        .style(S_BG);

    frame.render_widget(form_para, body);
}

// ─── Snapshot List ───────────────────────────────────────────────────

fn render_snapshot_list(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0), Constraint::Length(1)])
        .split(frame.size());
    let header = chunks[0];
    let body = chunks[1];
    let footer = chunks[2];

    let title = format!(" snapshots  ·  {} ", app.snapshots.len());
    render_title_bar(frame, header, &title, app);
    render_footer(frame, footer, app);

    if app.snapshots.is_empty() {
        render_centered(frame, body, &[
            ("⧖", MUTED),
            ("no snapshots yet", MUTED),
            ("", MUTED),
            ("run: clavus snapshot <path>", MUTED),
        ]);
        return;
    }

    let header_style = Style::new().fg(MUTED).bg(SURFACE).add_modifier(Modifier::BOLD);
    let snap_header = Row::new(vec![
        Span::raw("HASH"),
        Span::raw("MESSAGE"),
        Span::raw("BPM"),
        Span::raw("TRACKS"),
        Span::raw("SAMPLES"),
        Span::raw("TIMESTAMP"),
    ])
    .style(header_style)
    .height(1);

    let snap_rows: Vec<Row<'_>> = app.snapshots.iter().map(|s| {
        let ts = unix_to_relative(s.timestamp as i64);
        Row::new(vec![
            Span::raw(&s.hash[..8]).style(Style::new().fg(GOLD)),
            Span::raw(s.message.replace('\n', " ")),
            Span::raw(format!("{:.0}", s.bpm)),
            Span::raw(s.track_count.to_string()),
            Span::raw(s.sample_hashes.len().to_string()),
            Span::raw(ts),
        ])
        .style(Style::new().fg(TEXT).bg(SURFACE))
        .height(1)
    }).collect();

    let table = Table::new(snap_rows, &[
        Constraint::Length(8),   // HASH
        Constraint::Min(20),    // MESSAGE
        Constraint::Length(6),  // BPM
        Constraint::Length(7),  // TRACKS
        Constraint::Length(8),  // SAMPLES
        Constraint::Length(12), // TIMESTAMP
    ])
    .header(snap_header)
    .block(Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(BORDER)))
    .style(S_TEXT);

    frame.render_widget(table, body);
}

// ─── Sync ───────────────────────────────────────────────────────────

fn render_sync(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0), Constraint::Length(1)])
        .split(frame.size());
    let header = chunks[0];
    let body = chunks[1];
    let footer = chunks[2];

    render_title_bar(frame, header, "sync", app);
    render_footer(frame, footer, app);

    let conn_icon = if app.sync_state.connected { "◉" } else { "○" };
    let conn_color = if app.sync_state.connected { CYAN } else { MUTED };
    let status_text = if app.sync_state.connected {
        app.sync_state.relay_addr.as_deref().unwrap_or("connected")
    } else {
        "not connected"
    };

    let sync_lines = vec![
        l("", MUTED),
        line("  relay", MUTED, " 127.0.0.1:4477 ", MUTED),
        l("", MUTED),
        line("  status", MUTED, format!(" {} {}", conn_icon, status_text), conn_color),
        l("", MUTED),
        if let Some(t) = &app.sync_state.last_sync {
            line("  last sync", MUTED, format!(" {} ", t), TEXT)
        } else {
            line("  last sync", MUTED, "  never ", MUTED)
        },
        l("", MUTED),
        line("  key", ACCENT, " P ", TEXT),
        l("         push to relay", MUTED),
        line("  key", ACCENT, " G ", TEXT),
        l("         pull from relay", MUTED),
    ];

    let sync_para = Paragraph::new(sync_lines)
        .block(Block::default()
            .title(" sync ")
            .title_style(Style::new().fg(ACCENT).add_modifier(Modifier::BOLD))
            .borders(Borders::ALL)
            .border_style(Style::new().fg(BORDER)))
        .style(S_BG);

    frame.render_widget(sync_para, body);
}

// ─── Title bar ───────────────────────────────────────────────────────

fn render_title_bar(frame: &mut Frame, area: Rect, title: &str, app: &App) {
    // Background
    frame.render_widget(Paragraph::new("").style(S_SURFACE), area);

    // Left: app name in accent + title
    let left = Line::raw(" clavus ").style(S_ACCENT.add_modifier(Modifier::BOLD));
    let sep  = Line::raw(" · ").style(S_MUTED);
    let t    = Line::raw(title).style(S_TEXT);

    let left_para = Paragraph::new(vec![left, sep, t])
        .style(S_BG);
    frame.render_widget(left_para, area);

    // Right: key hint (dimmed)
    let hint = app.screen_hint();
    let hint_line = Line::raw(format!(" {} ", hint)).style(S_MUTED);
    let hint_para = Paragraph::new(vec![hint_line])
        .style(S_BG);
    let hint_width = hint.len() as u16 + 2;
    frame.render_widget(
        hint_para,
        Rect::new(area.right().saturating_sub(hint_width), area.y, hint_width, area.height),
    );

    // Bottom border line
    let border_line = Paragraph::new("")
        .style(Style::new().fg(BORDER));
    frame.render_widget(border_line, Rect::new(area.x, area.y + area.height - 1, area.width, 1));
}

// ─── Footer / status bar ─────────────────────────────────────────────

fn render_footer(frame: &mut Frame, area: Rect, app: &App) {
    frame.render_widget(Paragraph::new("").style(S_SURFACE), area);

    // Error or info message, otherwise key shortcuts
    let (content, color) = if let Some(e) = &app.error {
        (e.clone(), RED)
    } else if let Some(i) = &app.info {
        (i.clone(), GREEN)
    } else {
        let keys = app.footer_hint();
        (format!(" {} ", keys), MUTED)
    };

    let line = Line::raw(content).style(Style::new().fg(color));
    let para = Paragraph::new(vec![line]).style(S_BG);
    frame.render_widget(para, area);

    // Top border line
    let border_line = Paragraph::new("")
        .style(Style::new().fg(BORDER));
    frame.render_widget(border_line, Rect::new(area.x, area.y, area.width, 1));

    // Sync indicator dot (top-right of footer)
    let dot = if app.sync_state.connected { "●" } else { "" };
    let dot_color = if app.sync_state.connected { CYAN } else { BG };
    let dot_para = Paragraph::new(Line::raw(dot).style(Style::new().fg(dot_color)))
        .style(S_BG);
    frame.render_widget(dot_para, Rect::new(area.right().saturating_sub(1), area.y, 1, 1));
}

// ─── Helpers ─────────────────────────────────────────────────────────

fn l(text: &str, color: Color) -> Line<'_> {
    Line::raw(text).style(Style::new().fg(color))
}

fn line<S: AsRef<str>>(label: &str, label_color: Color, value: S, value_color: Color) -> Line<'static> {
    Line::from(vec![
        Span::raw(label.to_string()).style(Style::new().fg(label_color)),
        Span::raw(value.as_ref().to_string()).style(Style::new().fg(value_color)),
    ])
}

fn render_centered(frame: &mut Frame, area: Rect, lines: &[(&str, Color)]) {
    let content: Vec<Line<'_>> = lines.iter().map(|(t, c)| {
        Line::raw(*t).style(Style::new().fg(*c))
    }).collect();

    let para = Paragraph::new(content)
        .block(Block::default()
            .borders(Borders::ALL)
            .border_style(Style::new().fg(BORDER_DIM)))
        .style(S_BG);

    frame.render_widget(para, area);
}

fn render_centered_error(frame: &mut Frame, area: Rect, msg: &str) {
    render_centered(frame, area, &[
        ("✗", RED),
        ("", BG),
        (msg, MUTED),
    ]);
}

/// Convert Unix timestamp to relative time string.
fn unix_to_relative(secs: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let diff = now - secs;
    if diff < 60 {
        format!("{}s ago", diff)
    } else if diff < 3600 {
        format!("{}m ago", diff / 60)
    } else if diff < 86400 {
        format!("{}h ago", diff / 3600)
    } else if diff < 86400 * 30 {
        format!("{}d ago", diff / 86400)
    } else {
        format!("{}mo ago", diff / (86400 * 30))
    }
}

// ─── App screen hints ────────────────────────────────────────────────

impl App {
    pub fn screen_hint(&self) -> String {
        match self.screen {
            Screen::ProjectList  => "↑↓ select  ·  Enter open  ·  s sync  ·  q quit",
            Screen::ProjectDetail=> "c snap  ·  l cues  ·  n snapshots  ·  s sync  ·  Esc back",
            Screen::CueList      => "a add  ·  r resolve  ·  s skip  ·  f filter  ·  Esc back",
            Screen::CueAdd       => "Enter save  ·  Esc cancel",
            Screen::SnapshotList => "Esc back",
            Screen::Sync         => "P push  ·  G pull  ·  Esc back",
        }.to_string()
    }

    pub fn footer_hint(&self) -> String {
        match self.screen {
            Screen::ProjectList  => "↑↓ navigate  ·  Enter  open  ·  q quit",
            Screen::ProjectDetail=> "c snapshot  ·  l cues  ·  Esc",
            Screen::CueList      => "a add  ·  Esc",
            Screen::CueAdd       => "Enter save  ·  Esc cancel",
            Screen::SnapshotList => "Esc back",
            Screen::Sync         => "P push  ·  G pull  ·  Esc",
        }.to_string()
    }
}

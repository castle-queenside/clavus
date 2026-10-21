//! Clavus TUI rendering — Omarchy-inspired design.
//!
//! Warm blacks, layered panels, soft green accent, 4pt spacing grid.
//! Palette: bg #0b0c0d · panels #0e1012/#121517/#15181a · accent #74e08c

use crate::app::{App, Screen};
use crate::theme::*;
use ratatui::{
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Table, Row},
    Frame,
};

// ─── Entry point ─────────────────────────────────────────────────────

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
    let [header, body, footer] = split_v(frame.size());

    render_title_bar(frame, header, "projects", app);
    render_footer(frame, footer, app);

    if app.projects.is_empty() {
        render_idle(frame, body, app, "no projects", "Drop an .als or .bwproject file in your audio folder");
        return;
    }

    // ── Column headers ─────────────────────────────────────────────
    let h_style = Style::new().fg(MUTED).bg(PANEL_0).add_modifier(Modifier::BOLD);
    let header_row = Row::new(vec![
        Span::raw("TYPE"),
        Span::raw("NAME"),
        Span::raw("SNAPS"),
        Span::raw("BPM"),
        Span::raw("TRACKS"),
        Span::raw("MODIFIED"),
    ])
    .style(h_style)
    .height(1);

    // ── Project rows ──────────────────────────────────────────────
    let rows: Vec<Row<'_>> = app.projects.iter().enumerate().map(|(i, p)| {
        let sel = Some(i) == app.selected_project;
        let bg = if sel { ACCENT } else { PANEL_0 };
        let fg = if sel { BG } else { TEXT };

        let type_badge = if p.project_type == "Ableton" { "ALS" } else { "BW" };

        // Snapshot badge: count + short hash, or muted dash
        let (snap_text, snap_fg) = if p.snap_count > 0 {
            let hash = p
                .head
                .as_ref()
                .map(|h| clavus_core::short(h, 8))
                .unwrap_or_else(|| "?".to_string());
            (format!("{}·{}", p.snap_count, hash), ACCENT)
        } else {
            ("—".to_string(), MUTED)
        };

        let modified = unix_to_rel(p.modified_secs);

        Row::new(vec![
            Span::raw(type_badge),
            Span::raw(&p.name),
            Span::raw(snap_text).style(Style::new().fg(snap_fg)),
            Span::raw(format!("{:.0}", p.bpm)),
            Span::raw(p.track_count.to_string()),
            Span::raw(modified),
        ])
        .style(Style::new().fg(fg).bg(bg))
        .height(1)
    }).collect();

    let table = Table::new(rows, &[
        Constraint::Length(5),
        Constraint::Min(20),
        Constraint::Length(12),
        Constraint::Length(5),
        Constraint::Length(6),
        Constraint::Length(9),
    ])
    .header(header_row)
    .block(Block::default().borders(Borders::ALL).border_style(Style::new().fg(LINE)))
    .style(Style::new().fg(TEXT));

    frame.render_widget(table, body);
}

// ─── Project Detail ─────────────────────────────────────────────────

fn render_project_detail(frame: &mut Frame, app: &mut App) {
    let [header, body, footer] = split_v(frame.size());

    let project = match &app.project_detail {
        Some(p) => p,
        None => {
            render_idle(frame, body, app, "no project loaded", "select one from the list");
            return;
        }
    };

    let title = format!("{}  ·  {:.0} BPM  ·  {}", project.name, project.bpm, project.time_signature);
    render_title_bar(frame, header, &title, app);
    render_footer(frame, footer, app);

    // Two-column body
    let [tracks_col, info_col] = split_h(body, &[3, 2]);

    // ── Tracks table ─────────────────────────────────────────────
    let h_style = Style::new().fg(MUTED).bg(PANEL_0).add_modifier(Modifier::BOLD);
    let track_header = Row::new(vec![Span::raw("#"), Span::raw("NAME"), Span::raw("TYPE")])
        .style(h_style)
        .height(1);

    let track_rows: Vec<Row<'_>> = project.tracks.iter().enumerate().map(|(i, t)| {
        let icon = match t.track_type.as_str() {
            "Audio" => "🔊",
            "MIDI"  => "🎹",
            "Group" => "📂",
            _       => "•",
        };
        let audio = t.track_type == "Audio";
        Row::new(vec![
            Span::raw(format!("{:02}", i + 1)),
            Span::raw(format!("{} {}", icon, t.name)),
            Span::raw(&t.track_type),
        ])
        .style(Style::new().fg(if audio { TEXT } else { MUTED }).bg(PANEL_0))
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
        .border_style(Style::new().fg(LINE)))
    .style(Style::new().fg(TEXT));

    frame.render_widget(tracks_table, tracks_col);

    // ── Info + Markers panel ─────────────────────────────────────
    let [info_area, marker_area] = split_v_inner(info_col);

    // Info
    let info_lines = vec![
        l("", MUTED),
        row("TYPE",       MUTED, project.project_type.to_string()),
        row("BPM",        MUTED, format!("{:.1}", project.bpm)),
        row("SIGNATURE",  MUTED, project.time_signature.clone()),
        row("TRACKS",    MUTED, project.track_count.to_string()),
        row("MARKERS",   MUTED, project.markers.len().to_string()),
        if let Some(v) = &project.ableton_version {
            row("VERSION", MUTED, v.clone())
        } else {
            l("", MUTED)
        },
    ];

    let info_para = Paragraph::new(info_lines)
        .block(Block::default()
            .title(" info ")
            .title_style(Style::new().fg(MUTED).add_modifier(Modifier::BOLD))
            .borders(Borders::ALL)
            .border_style(Style::new().fg(LINE)))
        .style(S_BG);

    frame.render_widget(info_para, info_area);

    // Markers
    if project.markers.is_empty() {
        let empty = Paragraph::new(vec![l("", MUTED), l("  no markers", MUTED)])
            .block(Block::default()
                .title(" markers ")
                .title_style(Style::new().fg(MUTED).add_modifier(Modifier::BOLD))
                .borders(Borders::ALL)
                .border_style(Style::new().fg(LINE_LO)))
            .style(S_BG);
        frame.render_widget(empty, marker_area);
    } else {
        let marker_lines: Vec<Line<'_>> = project.markers.iter().map(|m| {
            Line::raw(format!("  {:>8}  {}", m.time, m.name)).style(Style::new().fg(WARM))
        }).collect();
        let marker_para = Paragraph::new(marker_lines)
            .block(Block::default()
                .title(format!(" markers  {} ", project.markers.len()))
                .title_style(Style::new().fg(MUTED).add_modifier(Modifier::BOLD))
                .borders(Borders::ALL)
                .border_style(Style::new().fg(LINE)))
            .style(S_BG);
        frame.render_widget(marker_para, marker_area);
    }
}

// ─── Cue List ───────────────────────────────────────────────────────

fn render_cue_list(frame: &mut Frame, app: &mut App) {
    let [header, body, footer] = split_v(frame.size());

    let title = format!(" cues  ·  {} ", app.cues.len());
    render_title_bar(frame, header, &title, app);
    render_footer(frame, footer, app);

    let filter = app.cue_filter.clone();
    let filtered: Vec<_> = app.cues.iter()
        .filter(|c| filter.is_none() || filter.as_ref().is_some_and(|f| c.status == *f))
        .collect();

    if filtered.is_empty() {
        let msg = if filter.is_some() { "no cues match filter" } else { "no cues yet" };
        let sub = if filter.is_some() { "clear the filter to see all" } else { "add one with a" };
        render_idle(frame, body, app, msg, sub);
        return;
    }

    // Filter indicator — small strip at top of body, not full body
    let filter_area = Rect::new(body.x, body.y, body.width, 1);
    if let Some(f) = &filter {
        let line = Paragraph::new(vec![Line::raw(format!("  filtered: {:?}", f))])
            .style(Style::new().fg(MUTED).bg(PANEL_0));
        frame.render_widget(line, filter_area);
    }

    let cue_rows: Vec<ListItem<'_>> = filtered.iter().map(|c| {
        let color = cue_status_color(&c.status);
        let icon  = cue_status_icon(&c.status);

        let line1 = Line::raw(format!(
            "{} {}  {:12}  {}",
            icon, c.status.as_str(), c.position, c.text,
        ))
        .style(Style::new().fg(color));

        let line2 = Line::raw(format!(
            "      {} · {} · {}",
            c.track_name,
            clavus_core::short(&c.id, 8),
            c.author
        ))
            .style(Style::new().fg(MUTED));

        ListItem::new(vec![line1, line2])
    }).collect();

    let list = List::new(cue_rows)
        .block(Block::default()
            .borders(Borders::ALL)
            .border_style(Style::new().fg(LINE)))
        .style(S_BG);

    let list_area = if filter.is_some() {
        Rect::new(body.x, body.y + 1, body.width, body.height.saturating_sub(1))
    } else {
        body
    };
    frame.render_widget(list, list_area);
}

// ─── Cue Add ─────────────────────────────────────────────────────────

fn render_cue_add(frame: &mut Frame, app: &mut App) {
    let [header, body, footer] = split_v(frame.size());

    render_title_bar(frame, header, "new cue", app);
    render_footer(frame, footer, app);

    let pos = app.input_buf.trim();
    let pos_display = if pos.is_empty() { "…" } else { pos };

    let form_lines = vec![
        l("", MUTED),
        l("  POSITION  (beats / bars.beats.sixteenths)", MUTED),
        l("", MUTED),
        Line::raw(format!("  {}█", pos_display))
            .style(Style::new().fg(ACCENT)),
        l("", MUTED),
        l("  TEXT", MUTED),
        l("  ", TEXT),
        l("", MUTED),
        l("  TRACK", MUTED),
        l("  Arrangement", TEXT),
        l("", MUTED),
        l("  author: tui", MUTED),
    ];

    let form = Paragraph::new(form_lines)
        .block(Block::default()
            .title(" add cue ")
            .title_style(Style::new().fg(ACCENT).add_modifier(Modifier::BOLD))
            .borders(Borders::ALL)
            .border_style(Style::new().fg(LINE)))
        .style(S_BG);

    frame.render_widget(form, body);
}

// ─── Snapshot List ───────────────────────────────────────────────────

fn render_snapshot_list(frame: &mut Frame, app: &mut App) {
    let [header, body, footer] = split_v(frame.size());

    let title = format!(" snapshots  ·  {} ", app.snapshots.len());
    render_title_bar(frame, header, &title, app);
    render_footer(frame, footer, app);

    if app.snapshots.is_empty() {
        render_idle(frame, body, app, idle_phrase(app.idle_index), "run: clavus snapshot <path>");
        return;
    }

    let h_style = Style::new().fg(MUTED).bg(PANEL_0).add_modifier(Modifier::BOLD);
    let snap_header = Row::new(vec![
        Span::raw("HASH"),
        Span::raw("MESSAGE"),
        Span::raw("BPM"),
        Span::raw("TRACKS"),
        Span::raw("SAMPLES"),
        Span::raw("TIMESTAMP"),
    ])
    .style(h_style)
    .height(1);

    let snap_rows: Vec<Row<'_>> = app.snapshots.iter().map(|s| {
        let ts = unix_to_rel(s.timestamp as i64);
        Row::new(vec![
            Span::raw(clavus_core::short(&s.hash, 8)).style(Style::new().fg(WARM)),
            Span::raw(s.message.replace('\n', " ")),
            Span::raw(format!("{:.0}", s.bpm)),
            Span::raw(s.track_count.to_string()),
            Span::raw(s.sample_hashes.len().to_string()),
            Span::raw(ts),
        ])
        .style(Style::new().fg(TEXT).bg(PANEL_0))
        .height(1)
    }).collect();

    let table = Table::new(snap_rows, &[
        Constraint::Length(8),
        Constraint::Min(20),
        Constraint::Length(6),
        Constraint::Length(7),
        Constraint::Length(8),
        Constraint::Length(12),
    ])
    .header(snap_header)
    .block(Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(LINE)))
    .style(Style::new().fg(TEXT));

    frame.render_widget(table, body);
}

// ─── Sync ───────────────────────────────────────────────────────────

fn render_sync(frame: &mut Frame, app: &mut App) {
    let [header, body, footer] = split_v(frame.size());

    render_title_bar(frame, header, "sync", app);
    render_footer(frame, footer, app);

    let conn = app.sync_state.connected;
    let conn_icon = if conn { "◉" } else { "○" };
    let conn_color = if conn { CYAN } else { MUTED };
    let status_text = if conn {
        app.sync_state.relay_addr.as_deref().unwrap_or("connected").to_string()
    } else {
        "not connected".to_string()
    };

    // ── Spectrum analyzer (Cliamp-inspired) ─────────────────────
    // 10-band LED bars with falling peak caps
    let spectrum = render_spectrum(app);

    let sync_lines = vec![
        l("", MUTED),
        l("  RELAY", MUTED),
        l("  127.0.0.1:4477", TEXT),
        l("", MUTED),
        l("  STATUS", MUTED),
        Line::raw(format!("  {} {}", conn_icon, status_text)).style(Style::new().fg(conn_color)),
        l("", MUTED),
        if let Some(t) = &app.sync_state.last_sync {
            Line::raw(format!("  last sync  {}", t)).style(Style::new().fg(MUTED))
        } else {
            Line::raw("  last sync  never").style(Style::new().fg(MUTED))
        },
        l("", MUTED),
        l("  PUSH  pull from relay", MUTED),
        l("  PULL  push to relay", MUTED),
        l("", MUTED),
    ];

    // Two-column: status left, spectrum right
    let [status_col, spectrum_col] = split_h(body, &[1, 1]);

    let sync_para = Paragraph::new(sync_lines)
        .block(Block::default()
            .title(" sync ")
            .title_style(Style::new().fg(ACCENT).add_modifier(Modifier::BOLD))
            .borders(Borders::ALL)
            .border_style(Style::new().fg(LINE)))
        .style(S_BG);

    frame.render_widget(sync_para, status_col);

    // Spectrum analyzer panel
    let spectrum_para = Paragraph::new(spectrum)
        .block(Block::default()
            .title(" spectrum ")
            .title_style(Style::new().fg(MUTED).add_modifier(Modifier::BOLD))
            .borders(Borders::ALL)
            .border_style(Style::new().fg(LINE)))
        .style(Style::new().fg(ACCENT).bg(PANEL_0));

    frame.render_widget(spectrum_para, spectrum_col);
}

// ─── Spectrum analyzer ────────────────────────────────────────────────
// Cliamp-inspired: 10 bars with falling peak caps. Animated via app state.

fn render_spectrum(app: &App) -> Vec<Line<'static>> {
    // Simulated 10-band EQ levels. In a real implementation these would
    // come from audio analysis. For now, generate pseudo-random levels
    // that look musical.
    let bands: [f64; 10] = if app.sync_state.connected {
        use std::time::SystemTime;
        let seed = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        // Pseudo-musical levels: low bands stronger, high bands weaker
        let mut levels = [0.0f64; 10];
        for (i, l) in levels.iter_mut().enumerate() {
            let base = (10 - i) as f64 * 0.08;
            let noise = ((seed.wrapping_mul((i + 1) as u64 * 31337)) % 100) as f64 * 0.01;
            *l = (base + noise).clamp(0.05, 1.0);
        }
        levels
    } else {
        [0.05f64; 10]
    };

    let bar_chars: &[&str] = &[" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];

    let mut lines = Vec::new();

    // Top: peak indicators (1 line)
    let peak_line: String = bands.iter().map(|l| {
        let level = *l;
        let peak = if level > 0.7 { "▂" } else if level > 0.4 { "▃" } else { " " };
        peak.to_string()
    }).collect();
    lines.push(Line::raw(peak_line).style(Style::new().fg(MUTED)));

    // Bottom: 8-segment LED bars (1 line)
    let bar_line: String = bands.iter().map(|l| {
        let seg = ((l * 8.0).round() as usize).min(8);
        bar_chars[seg].to_string()
    }).collect();
    lines.push(Line::raw(bar_line).style(Style::new().fg(ACCENT)));

    lines
}

// ─── Title bar ───────────────────────────────────────────────────────

fn render_title_bar(frame: &mut Frame, area: Rect, title: &str, app: &App) {
    frame.render_widget(Paragraph::new("").style(Style::new().bg(PANEL_0)), area);

    // Left: "clavus" in accent + title
    let title_line = {
        let mut l = Line::default();
        l.spans.push(Span::raw(" clavus ").style(Style::new().fg(BG).bg(ACCENT).add_modifier(Modifier::BOLD)));
        l.spans.push(Span::raw(" · ").style(Style::new().fg(MUTED)));
        l.spans.push(Span::raw(title).style(Style::new().fg(TEXT)));
        l
    };
    let para = Paragraph::new(vec![title_line]).style(S_BG);
    frame.render_widget(para, area);

    // Right: key hint
    let hint = app.screen_hint();
    let hint_line = Line::raw(format!(" {} ", hint)).style(Style::new().fg(MUTED));
    let hint_para = Paragraph::new(vec![hint_line]).style(S_BG);
    let hint_w = hint.len() as u16 + 2;
    frame.render_widget(hint_para, Rect::new(area.right().saturating_sub(hint_w), area.y, hint_w, area.height));

    // Bottom border
    let border = Paragraph::new("").style(Style::new().fg(LINE));
    frame.render_widget(border, Rect::new(area.x, area.y + area.height - 1, area.width, 1));
}

// ─── Footer / status bar ─────────────────────────────────────────────

fn render_footer(frame: &mut Frame, area: Rect, app: &App) {
    frame.render_widget(Paragraph::new("").style(Style::new().bg(PANEL_0)), area);

    let (content, color) = if let Some(e) = &app.error {
        (e.clone(), RED)
    } else if let Some(i) = &app.info {
        (i.clone(), ACCENT)
    } else {
        (format!(" {} ", app.footer_hint()), MUTED)
    };

    let line = Line::raw(content).style(Style::new().fg(color));
    let para = Paragraph::new(vec![line]).style(S_BG);
    frame.render_widget(para, area);

    // Top border
    let border = Paragraph::new("").style(Style::new().fg(LINE));
    frame.render_widget(border, Rect::new(area.x, area.y, area.width, 1));

    // Sync dot (top-right of footer)
    let dot = if app.sync_state.connected { "●" } else { "" };
    let dot_color = if app.sync_state.connected { CYAN } else { BG };
    let dot_para = Paragraph::new(Line::raw(dot).style(Style::new().fg(dot_color))).style(S_BG);
    frame.render_widget(dot_para, Rect::new(area.right().saturating_sub(1), area.y, 1, 1));
}

// ─── Idle state ────────────────────────────────────────────────────────

fn render_idle(frame: &mut Frame, area: Rect, _app: &App, msg: &str, sub: &str) {
    let lines = vec![
        l("", MUTED),
        Line::raw(format!("  {}", msg)).style(Style::new().fg(MUTED)),
        l("", MUTED),
        Line::raw(format!("  {}", sub)).style(Style::new().fg(MUTED)),
        l("", MUTED),
    ];
    let para = Paragraph::new(lines)
        .block(Block::default()
            .borders(Borders::ALL)
            .border_style(Style::new().fg(LINE_LO)))
        .style(S_BG);
    frame.render_widget(para, area);
}

// ─── Helpers ─────────────────────────────────────────────────────────

/// Two-arg line: plain colored text.
fn l(text: &str, color: Color) -> Line<'_> {
    Line::raw(text.to_string()).style(Style::new().fg(color))
}

/// Labeled row: muted label + bright value.
fn row(label: &str, lc: Color, value: impl AsRef<str>) -> Line<'_> {
    let mut line = Line::default();
    line.spans.push(Span::raw(label.to_string()).style(Style::new().fg(lc)));
    line.spans.push(Span::raw(format!("  {}", value.as_ref())).style(Style::new().fg(TEXT)));
    line
}

/// Split area vertically into 3 parts: header (1 row), body (flex), footer (1 row).
fn split_v(area: Rect) -> [Rect; 3] {
    let h = area.height;
    [
        Rect::new(area.x, area.y,           area.width, 1),
        Rect::new(area.x, area.y + 1,       area.width, h.saturating_sub(2)),
        Rect::new(area.x, area.y + h - 1,   area.width, 1),
    ]
}

/// Split area horizontally into 2 parts by percentage ratio.
fn split_h(area: Rect, ratio: &[usize; 2]) -> [Rect; 2] {
    let total = ratio[0] + ratio[1];
    if total == 0 {
        // A zero ratio would divide by zero; give everything to the
        // first pane rather than panicking.
        return [
            Rect::new(area.x, area.y, area.width, area.height),
            Rect::new(area.x, area.y, 0, area.height),
        ];
    }
    let w0 = u16::try_from(area.width as usize * ratio[0] / total).unwrap_or(area.width);
    [
        Rect::new(area.x, area.y, w0, area.height),
        Rect::new(area.x + w0, area.y, area.width.saturating_sub(w0), area.height),
    ]
}

/// Split area vertically into 2 equal parts.
fn split_v_inner(area: Rect) -> [Rect; 2] {
    let h0 = area.height / 2;
    [
        Rect::new(area.x, area.y, area.width, h0),
        Rect::new(area.x, area.y + h0, area.width, area.height.saturating_sub(h0)),
    ]
}

/// Convert Unix timestamp to relative time string.
fn unix_to_rel(secs: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let d = now - secs;
    if d < 5      { "just now".to_string() }
    else if d < 60    { format!("{}s ago", d) }
    else if d < 3600  { format!("{}m ago", d / 60) }
    else if d < 86400  { format!("{}h ago", d / 3600) }
    else if d < 2592000 { format!("{}d ago", d / 86400) }
    else                { format!("{}mo ago", d / 2592000) }
}

// ─── App hints ────────────────────────────────────────────────────────

impl App {
    pub fn screen_hint(&self) -> String {
        match self.screen {
            Screen::ProjectList  => "↑↓ select  ·  Enter open  ·  s sync  ·  q quit",
            Screen::ProjectDetail=> "c snap  ·  l cues  ·  n snapshots  ·  s sync  ·  Esc back",
            Screen::CueList      => "↑↓ navigate  ·  a add  ·  r resolve  ·  f filter  ·  Esc back",
            Screen::CueAdd       => "Enter save  ·  Esc cancel",
            Screen::SnapshotList => "↑↓ navigate  ·  Esc back",
            Screen::Sync         => "P push  ·  G pull  ·  Esc back",
        }.to_string()
    }

    pub fn footer_hint(&self) -> String {
        match self.screen {
            Screen::ProjectList  => "↑↓ navigate  ·  Enter  open  ·  q quit",
            Screen::ProjectDetail=> "c snapshot  ·  l cues  ·  Esc",
            Screen::CueList      => "↑↓ navigate  ·  a add  ·  Esc",
            Screen::CueAdd       => "Enter save  ·  Esc cancel",
            Screen::SnapshotList => "↑↓ navigate  ·  Esc",
            Screen::Sync         => "P push  ·  G pull  ·  Esc",
        }.to_string()
    }
}

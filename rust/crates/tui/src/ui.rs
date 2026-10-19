//! Clavus TUI rendering with Ratatui.

use crate::app::{App, Screen};
use clavus_core::types::CueStatus;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Table, Row, Gauge},
    Frame,
};

/// Render the main frame.
pub fn render(frame: &mut Frame, app: &mut App) {
    match app.screen {
        Screen::ProjectList => render_project_list(frame, app),
        Screen::ProjectDetail => render_project_detail(frame, app),
        Screen::CueList => render_cue_list(frame, app),
        Screen::CueAdd => render_cue_add(frame, app),
        Screen::SnapshotList => render_snapshot_list(frame, app),
        Screen::Sync => render_sync(frame, app),
    }
}

// ─── Project List ────────────────────────────────────────────────────

fn render_project_list(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // header
            Constraint::Min(0),       // project list
            Constraint::Length(3),  // status bar
        ])
        .split(frame.size());

    render_header(frame, chunks[0], "Projects", app);
    render_status_bar(frame, chunks[2], app);

    let items: Vec<ListItem> = app.projects
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let name = if Some(i) == app.selected_project {
                format!("> {}", p.name)
            } else {
                p.name.clone()
            };
            let info = format!("[{}] {} tracks · {:.1} BPM", p.project_type, p.track_count, p.bpm);
            ListItem::new(vec![
                Line::raw(name).style(if Some(i) == app.selected_project {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                }),
                Line::raw(info).style(Color::DarkGray),
            ])
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().title("Projects").borders(Borders::ALL))
        .style(Style::default());

    frame.render_widget(list, chunks[1]);

    if let Some(idx) = app.selected_project {
        render_scroll_hints(frame, chunks[1], app.projects.len(), idx);
    }
}

// ─── Project Detail ─────────────────────────────────────────────────

fn render_project_detail(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
            Constraint::Length(3),
        ])
        .split(frame.size());

    let project = match &app.project_detail {
        Some(p) => p,
        None => {
            render_error(frame, chunks[1], "No project loaded");
            return;
        }
    };

    render_header(frame, chunks[0], &project.name, app);

    // Two-column: tracks on left, info on right
    let detail = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(chunks[1]);

    // Tracks table
    let track_rows: Vec<Row> = project.tracks.iter().enumerate().map(|(i, t)| {
        let icon = match t.track_type.as_str() {
            "Audio" => "🔊",
            "MIDI" => "🎹",
            "Group" => "📂",
            _ => "•",
        };
        Row::new(vec![
            format!("{:02}", i + 1),
            icon.to_string(),
            t.name.clone(),
            t.track_type.clone(),
        ])
    }).collect();

    let tracks_table = Table::new(track_rows, &[Constraint::Length(3), Constraint::Length(3), Constraint::Percentage(60), Constraint::Length(10)])
        .block(Block::default().title(format!("Tracks ({})", project.tracks.len())).borders(Borders::ALL));

    frame.render_widget(tracks_table, detail[0]);

    // Project info
    let info_text = vec![
        Line::from(vec![Span::raw("Type:  "), Span::raw(project.project_type.to_string())]),
        Line::from(vec![Span::raw("BPM:   "), Span::raw(format!("{:.1}", project.bpm))]),
        Line::from(vec![Span::raw("Time:  "), Span::raw(&project.time_signature)]),
        Line::from(vec![Span::raw("Tracks:"), Span::raw(format!("{}", project.track_count))]),
        Line::from(vec![Span::raw("Markers:"), Span::raw(format!("{}", project.markers.len()))]),
        if let Some(v) = &project.ableton_version {
            Line::from(vec![Span::raw("Version:"), Span::raw(v.clone())])
        } else {
            Line::from(vec![Span::raw("")])
        },
    ];

    let info_para = Paragraph::new(info_text)
        .block(Block::default().title("Info").borders(Borders::ALL))
        .style(Style::default());

    frame.render_widget(info_para, detail[1]);

    // Markers
    if !project.markers.is_empty() {
        let marker_text: Vec<Line> = project.markers.iter().map(|m| {
            Line::raw(format!("  {}  {}", m.time, m.name))
        }).collect();
        let marker_para = Paragraph::new(marker_text)
            .block(Block::default().title(format!("Markers ({})", project.markers.len())).borders(Borders::ALL));
        frame.render_widget(marker_para, chunks[2]);
    } else {
        let marker_para = Paragraph::new("  No markers").block(Block::default().title("Markers").borders(Borders::ALL));
        frame.render_widget(marker_para, chunks[2]);
    }

    render_status_bar(frame, chunks[3], app);
}

// ─── Cue List ──────────────────────────────────────────────────────

fn render_cue_list(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(frame.size());

    let title = format!("Cues — {}", app.selected_entry().map(|e| e.name.as_str()).unwrap_or("?"));
    render_header(frame, chunks[0], &title, app);
    render_status_bar(frame, chunks[2], app);

    let filter = app.cue_filter.clone();
    let filtered: Vec<_> = app.cues.iter().filter(|c| {
        filter.as_ref().map_or(true, |f| c.status == *f)
    }).collect();

    let items: Vec<ListItem> = filtered.iter().map(|c| {
        let status_icon = match c.status {
            CueStatus::Pending => "○",
            CueStatus::Deferred => "◔",
            CueStatus::Resolved => "●",
            CueStatus::Skipped => "✗",
            CueStatus::Archived => "▣",
        };
        let color = match c.status {
            CueStatus::Pending => Color::Yellow,
            CueStatus::Deferred => Color::DarkGray,
            CueStatus::Resolved => Color::Green,
            CueStatus::Skipped => Color::Red,
            CueStatus::Archived => Color::Magenta,
        };
        let line1 = Line::raw(format!("{} [{}] {} @ {}  ({})",
            status_icon,
            c.status.as_str(),
            c.text,
            c.position,
            c.author
        )).style(color);
        let line2 = Line::raw(format!("    {} · {}", c.track_name, &c.id[..8])).style(Color::DarkGray);
        ListItem::new(vec![line1, line2])
    }).collect();

    let list = List::new(items)
        .block(Block::default()
            .title(format!("Cues ({})", filtered.len()))
            .borders(Borders::ALL))
        .style(Style::default());

    frame.render_widget(list, chunks[1]);
}

// ─── Cue Add ────────────────────────────────────────────────────────

fn render_cue_add(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(frame.size());

    render_header(frame, chunks[0], "Add Cue", app);

    let prompt = Paragraph::new("Position (beats):");
    frame.render_widget(prompt, chunks[1]);

    let input = Paragraph::new(app.input_buf.as_str())
        .block(Block::default().title("Cue text").borders(Borders::ALL))
        .style(Style::default().fg(Color::White));
    frame.render_widget(input, chunks[3]);

    render_status_bar(frame, chunks[4], app);
}

// ─── Snapshot List ──────────────────────────────────────────────────

fn render_snapshot_list(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(frame.size());

    let title = format!("Snapshots — {}", app.selected_entry().map(|e| e.name.as_str()).unwrap_or("?"));
    render_header(frame, chunks[0], &title, app);
    render_status_bar(frame, chunks[2], app);

    let para = Paragraph::new("No snapshots yet.\n\n:c to create one")
        .block(Block::default().title("Snapshots").borders(Borders::ALL));
    frame.render_widget(para, chunks[1]);
}

// ─── Sync ───────────────────────────────────────────────────────────

fn render_sync(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(frame.size());

    render_header(frame, chunks[0], "Sync", app);

    let status = if app.sync_state.connected {
        format!("Connected to {}", app.sync_state.relay_addr.as_deref().unwrap_or("?"))
    } else {
        "Not connected".to_string()
    };

    let lines = vec![
        Line::raw("Clavus Sync"),
        Line::raw(""),
        Line::raw(format!("Status: {}", if app.sync_state.connected { "🟢 Connected" } else { "⚫ Offline" })),
        Line::raw(format!("Relay:  {}", status)),
        if let Some(t) = &app.sync_state.last_sync {
            Line::raw(format!("Last:   {}", t))
        } else {
            Line::raw("Last:   never".to_string())
        },
    ];

    let para = Paragraph::new(lines)
        .block(Block::default().title("Sync Status").borders(Borders::ALL));
    frame.render_widget(para, chunks[1]);
    render_status_bar(frame, chunks[2], app);
}

// ─── Shared ─────────────────────────────────────────────────────────

fn render_header(frame: &mut Frame, area: Rect, title: &str, app: &App) {
    let (left, right) = {
        let _chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
            .split(area);

        let title_line = Line::from(vec![
            Span::raw(" clavus "),
            Span::raw(title),
        ]).style(Style::new().fg(Color::White).bg(Color::Blue).add_modifier(Modifier::BOLD));

        let hint_line = Line::from(vec![
            Span::raw(format!(" {} ", app.screen_help())),
        ]).style(Color::DarkGray);

        (title_line, hint_line)
    };

    let header = ratatui::widgets::Paragraph::new(vec![left])
        .block(Block::default().borders(Borders::TOP | Borders::LEFT | Borders::RIGHT));
    frame.render_widget(header, area);

    let hint = ratatui::widgets::Paragraph::new(vec![right])
        .block(Block::default().borders(Borders::TOP | Borders::RIGHT));
    frame.render_widget(hint, area);
}

fn render_status_bar(frame: &mut Frame, area: Rect, app: &App) {
    let status = if let Some(e) = &app.error {
        vec![Line::from(vec![Span::raw(" ✗ "), Span::raw(e)]).style(Style::new().fg(Color::Red).bg(Color::Black))]
    } else if let Some(i) = &app.info {
        vec![Line::from(vec![Span::raw(" ✓ "), Span::raw(i)]).style(Style::new().fg(Color::Green).bg(Color::Black))]
    } else {
        vec![Line::from(vec![Span::raw(format!(" {} · {} ", app.screen_help_short(), app.projects.len()))]).style(Color::DarkGray)]
    };

    let para = Paragraph::new(status)
        .block(Block::default().borders(Borders::BOTTOM | Borders::LEFT | Borders::RIGHT));
    frame.render_widget(para, area);
}

fn render_scroll_hints(frame: &mut Frame, area: Rect, total: usize, selected: usize) {
    // Scrollbar hint
    if total > 1 {
        let ratio = selected as f64 / (total - 1) as f64;
        let gauge = Gauge::default()
            .ratio(ratio)
            .gauge_style(Style::default().fg(Color::Blue));
        frame.render_widget(gauge, Rect::new(area.right() - 2, area.y + 1, 1, area.height.saturating_sub(2)));
    }
}

// Dummy screen_help so build succeeds — real implementation uses App::screen_help
fn _dummy() {
    let _ = Screen::ProjectList;
}

fn render_error(frame: &mut Frame, area: Rect, msg: &str) {
    let para = Paragraph::new(msg)
        .block(Block::default().title("Error").borders(Borders::ALL))
        .style(Style::default().fg(Color::Red));
    frame.render_widget(para, area);
}

// ─── App helpers ────────────────────────────────────────────────────

impl App {
    pub fn screen_help(&self) -> String {
        match self.screen {
            Screen::ProjectList => "↑↓ navigate · Enter open · c create · s sync · q quit",
            Screen::ProjectDetail => "c snapshot · l cues · p push · r relay · Esc back",
            Screen::CueList => "a add · r resolve · s skip · f filter · Esc back",
            Screen::CueAdd => "Enter save · Esc cancel",
            Screen::SnapshotList => "Enter inspect · Esc back",
            Screen::Sync => "p push · P pull · Esc back",
        }.to_string()
    }

    pub fn screen_help_short(&self) -> String {
        match self.screen {
            Screen::ProjectList => "↑↓ Enter c s q",
            Screen::ProjectDetail => "c l p Esc",
            Screen::CueList => "a r s f Esc",
            Screen::CueAdd => "Enter Esc",
            Screen::SnapshotList => "Enter Esc",
            Screen::Sync => "p P Esc",
        }.to_string()
    }
}

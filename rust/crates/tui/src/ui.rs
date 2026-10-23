//! Clavus TUI rendering — Omarchy-inspired design.
//!
//! Warm blacks, layered panels, soft green accent, 4pt spacing grid.
//! Palette: bg #0b0c0d · panels #0e1012/#121517/#15181a · accent #74e08c

use crate::app::{App, Screen, ACTIVITY_CELLS};
use crate::theme::*;
use ratatui::{
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, List, ListItem, Paragraph, Table, Row},
    Frame,
};

// ─── Entry point ─────────────────────────────────────────────────────

pub fn render(frame: &mut Frame, app: &mut App) {
    // A terminal that is too small to hold the interface is not a
    // reason to die. The layout below subtracts and divides, and a
    // window of two rows used to panic — silently, because the panic
    // hook swallowed the message. The user is told the window is too
    // small and the app keeps running.
    let size = frame.size();
    if size.width < 60 || size.height < 10 {
        // The message is sized to fit: a very narrow window still gets
        // a word, so the user is never left with a blank screen and no
        // explanation.
        let msg = if size.width < 20 {
            "too small".to_string()
        } else {
            format!(
                "  too small ({}x{}) — Clavus needs 60x10",
                size.width, size.height
            )
        };
        let area = Rect::new(size.x, size.y, size.width, 1);
        frame.render_widget(
            Paragraph::new(msg).style(Style::new().fg(MUTED).bg(BG)),
            area,
        );
        return;
    }

    // The canvas is painted before anything else. A widget that does
    // not set a background of its own would otherwise show the
    // *terminal's* background through, and the screen would then be
    // half Clavus's warm black and half whatever the terminal was
    // configured with — which reads as a patchy screen over SSH, and
    // changes colour when the terminal is reconfigured.
    frame.render_widget(Block::default().style(S_BG), size);

    match app.screen {
        Screen::ProjectList  => render_project_list(frame, app),
        Screen::ProjectDetail=> render_project_detail(frame, app),
        Screen::CueList      => render_cue_list(frame, app),
        Screen::CueAdd       => render_cue_add(frame, app),
        Screen::SnapshotList  => render_snapshot_list(frame, app),
        Screen::Sync         => render_sync(frame, app),
        Screen::RelayInput   => render_relay_input(frame, app),
    }
}

// ─── Relay address entry ────────────────────────────────────────────

fn render_relay_input(frame: &mut Frame, app: &mut App) {
    let [header, body, footer] = split_v(frame.size());

    render_title_bar(frame, header, "relay address", app);
    render_footer(frame, footer, app);

    let addr = app.input_buf.trim();
    let display = if addr.is_empty() { "…" } else { addr };

    let form_lines = vec![
        l("", MUTED),
        l("  RELAY ADDRESS  (host:port)", MUTED),
        l("", MUTED),
        Line::raw(format!("  {}█", display)).style(Style::new().fg(ACCENT)),
        l("", MUTED),
        l("  e.g. 192.168.1.10:4477", MUTED),
    ];

    let form = Paragraph::new(form_lines)
        .block(Block::default()
            .title(" connect ")
            .title_style(Style::new().fg(ACCENT).add_modifier(Modifier::BOLD))
            .borders(Borders::ALL)
            .border_style(Style::new().fg(LINE)))
        .style(S_BG);

    frame.render_widget(form, body);
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

    let visible = app.visible_indices();
    if visible.is_empty() {
        let sub = format!("Nothing matches “{}”", app.filter);
        render_idle(frame, body, app, "no match", &sub);
        return;
    }

    // The strip that says which project you are looking at. It is
    // always the same height, whether or not anything is selected, so
    // the list beneath it never moves.
    let [strip, list_area] = split_off(body, 1);
    render_focus_strip(frame, strip, app);

    let cols = columns_for(list_area.width);

    // ── Column headers ─────────────────────────────────────────────
    let h_style = Style::new().fg(MUTED).bg(PANEL_0).add_modifier(Modifier::BOLD);
    let mut header_cells = vec![Span::raw("TYPE"), Span::raw("NAME")];
    if cols.folder {
        header_cells.push(Span::raw("FOLDER"));
    }
    header_cells.push(Span::raw("SNAPS"));
    if cols.bpm {
        header_cells.push(Span::raw("BPM"));
    }
    if cols.tracks {
        header_cells.push(Span::raw("TRACKS"));
    }
    header_cells.push(Span::raw("WORK"));
    if cols.open {
        header_cells.push(Span::raw("OPEN"));
    }
    if cols.age {
        header_cells.push(Span::raw("AGE"));
    }
    let header_row = Row::new(header_cells).style(h_style).height(1);

    // ── Project rows ──────────────────────────────────────────────
    let now = unix_now();
    let rows: Vec<Row<'_>> = visible
        .iter()
        .map(|&pi| {
            let p = &app.projects[pi];
            let sel = app.selected_project == Some(pi);
            // A whole row filled with the full accent is a bright block
            // that travels as the cursor moves. The selected row is raised
            // instead — a dark green that is plainly a different surface
            // from the panel around it.
            let bg = if sel { ACCENT_DIM } else { PANEL_0 };
            let fg = TEXT;

            let type_badge = if p.project_type == "Ableton" { "ALS" } else { "BW" };
            // The marker sits in the type badge's own column, so the text
            // does not move when the row is selected. A row that shifts
            // under the cursor reads as a glitch.
            let type_cell = if sel {
                format!("\u{258c}{}", type_badge)
            } else {
                format!(" {}", type_badge)
            };

            // Snapshot badge: count + short hash, or muted dash
            let (snap_text, snap_fg) = if p.snap_count > 0 {
                let hash = p
                    .head
                    .as_ref()
                    .map(|h| clavus_core::short(h, 8))
                    .unwrap_or_else(|| "?".to_string());
                (format!("{}\u{00b7}{}", p.snap_count, hash), ACCENT)
            } else {
                ("\u{2014}".to_string(), MUTED)
            };

            let mut cells: Vec<Cell<'_>> = vec![
                Cell::from(Span::raw(type_cell).style(Style::new().fg(fg).bg(bg))),
                Cell::from(Span::raw(&p.name).style(Style::new().fg(fg).bg(bg))),
            ];
            if cols.folder {
                cells.push(Cell::from(
                    Span::raw(&p.folder).style(Style::new().fg(MUTED).bg(bg)),
                ));
            }
            cells.push(Cell::from(
                Span::raw(snap_text).style(Style::new().fg(snap_fg).bg(bg)),
            ));
            if cols.bpm {
                cells.push(Cell::from(
                    Span::raw(format!("{:.0}", p.bpm)).style(Style::new().fg(fg).bg(bg)),
                ));
            }
            if cols.tracks {
                cells.push(Cell::from(
                    Span::raw(p.track_count.to_string()).style(Style::new().fg(fg).bg(bg)),
                ));
            }

            // The shape of the project's own history, oldest on the
            // left. Each bar carries its own colour, so the cell is a
            // run of spans rather than one piece of text.
            //
            // An empty span is drawn as the shortest bar in the
            // quietest colour rather than as a space: the column
            // then reads as one chart with a baseline, and a project
            // with no history at all reads as a flat line.
            cells.push(Cell::from(Line::from(
                p.activity
                    .iter()
                    .map(|&h| {
                        let height = if h == 0 { 1 } else { h };
                        Span::raw(crate::spark::bar(height).to_string()).style(Style::new().fg(
                            if h == 0 { LINE_LO } else { spark_color(h) },
                        ).bg(bg))
                    })
                    .collect::<Vec<Span<'_>>>(),
            )));

            // Notes left open. A note you never closed is the clearest
            // sign the project is still in progress.
            if cols.open {
                let (text, colour) = if p.open_cues > 0 {
                    (format!("\u{25cb}{}", p.open_cues), ACCENT)
                } else {
                    ("\u{2014}".to_string(), MUTED)
                };
                cells.push(Cell::from(
                    Span::raw(text).style(Style::new().fg(colour).bg(bg)),
                ));
            }

            // How long ago the file itself was last touched. A project
            // changed today is one being worked on right now, and it is
            // warm rather than grey.
            if cols.age {
                let (text, colour) = age_text(p.modified_secs, now);
                cells.push(Cell::from(
                    Span::raw(text).style(Style::new().fg(colour).bg(bg)),
                ));
            }

            Row::new(cells)
                .style(Style::new().fg(fg).bg(bg))
                .height(1)
        })
        .collect();

    let mut widths = vec![
        Constraint::Length(5),
        Constraint::Length(cols.name_width(list_area.width)),
    ];
    if cols.folder {
        widths.push(Constraint::Length(18));
    }
    widths.push(Constraint::Length(12));
    if cols.bpm {
        widths.push(Constraint::Length(5));
    }
    if cols.tracks {
        widths.push(Constraint::Length(6));
    }
    widths.push(Constraint::Length(ACTIVITY_CELLS as u16));
    if cols.open {
        widths.push(Constraint::Length(5));
    }
    if cols.age {
        widths.push(Constraint::Length(5));
    }

    let table = Table::new(rows, &widths)
        .header(header_row)
        .column_spacing(COLUMN_SPACING)
        .block(Block::default()
            .borders(Borders::ALL)
            .border_style(Style::new().fg(LINE))
            // The panel's own surface, so the border and the empty space
            // below the list are the same colour as the rows.
            .style(Style::new().bg(PANEL_0)))
        .style(Style::new().fg(TEXT).bg(PANEL_0));

    frame.render_widget(table, list_area);
}

// ─── The focus strip ─────────────────────────────────────────────────
//
// Which project you are looking at, and what state it is in. It sits
// between the title bar and the list, at a fixed height, so it can
// change its contents without moving anything.
//
// This is the signal that a project is being worked on. It is *not* a
// notification: a notification appears, goes away, and competes with
// whatever else the bar is saying. This is always there, and it
// belongs to the row you are on.

fn render_focus_strip(frame: &mut Frame, area: Rect, app: &App) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    frame.render_widget(Paragraph::new("").style(Style::new().bg(PANEL_1)), area);

    let mut spans: Vec<Span<'static>> = vec![Span::raw(" \u{25b8} ").style(Style::new().fg(ACCENT).bg(PANEL_1))];

    match app.selected_entry() {
        Some(p) => {
            spans.push(Span::raw(p.name.clone()).style(Style::new().fg(TEXT).bg(PANEL_1)));
            spans.push(Span::raw("  ").style(Style::new().fg(MUTED).bg(PANEL_1)));
            spans.push(
                Span::raw(format!("{} snapshot{}", p.snap_count, plural(p.snap_count)))
                    .style(Style::new().fg(ACCENT).bg(PANEL_1)),
            );
            if p.open_cues > 0 {
                spans.push(Span::raw("  ").style(Style::new().fg(MUTED).bg(PANEL_1)));
                spans.push(
                    Span::raw(format!("\u{25cb}{} open", p.open_cues))
                        .style(Style::new().fg(ACCENT).bg(PANEL_1)),
                );
            }
            let (age, colour) = age_text(p.modified_secs, unix_now());
            spans.push(Span::raw("  ").style(Style::new().fg(MUTED).bg(PANEL_1)));
            spans.push(Span::raw(format!("edited {}", age)).style(Style::new().fg(colour).bg(PANEL_1)));
        }
        None => {
            spans.push(
                Span::raw("no project selected \u{2014} \u{2191}\u{2193} to choose")
                    .style(Style::new().fg(MUTED).bg(PANEL_1)),
            );
        }
    }

    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::new().bg(PANEL_1)),
        area,
    );
}

// ─── The columns that fit ───────────────────────────────────────────
//
// Clavus runs in a terminal as narrow as sixty columns, and a table
// with a column for everything is unreadable in one. The columns are
// dropped from the least interesting inwards as the window narrows,
// and the ones that stay are the ones that say what is going on: what
// the project is called, how much history it has, the shape of that
// history, and whether it is still open.

struct Columns {
    folder: bool,
    bpm: bool,
    tracks: bool,
    open: bool,
    age: bool,
}

/// The width of the name column when nothing else is left for it.
const NAME_MIN: u16 = 12;

/// The width of the panel's left and right borders.
const BORDERS: u16 = 2;

fn columns_for(width: u16) -> Columns {
    // The always-there columns are TYPE (5), NAME (12), SNAPS (12) and
    // WORK (8), with a space between each column and a border either
    // side — 42 columns in all. Each column below adds to that, and
    // each is only kept when there is still room for the name.
    let w = width as usize;
    Columns {
        age: w >= 48,
        open: w >= 54,
        folder: w >= 73,
        bpm: w >= 79,
        tracks: w >= 86,
    }
}

/// The space between two columns of the table.
const COLUMN_SPACING: u16 = 1;

impl Columns {
    /// How many columns the table has.
    fn count(&self) -> usize {
        4 + self.folder as usize + self.bpm as usize + self.tracks as usize
            + self.open as usize + self.age as usize
    }

    /// The width every column but the name takes.
    fn fixed(&self) -> u16 {
        let mut total = 5 + NAME_MIN + 12 + ACTIVITY_CELLS as u16;
        if self.folder {
            total += 18;
        }
        if self.bpm {
            total += 5;
        }
        if self.tracks {
            total += 6;
        }
        if self.open {
            total += 5;
        }
        if self.age {
            total += 5;
        }
        total
    }

    /// The width left for the name.
    ///
    /// The name takes what is left over rather than asking for a
    /// minimum and hoping. A chart that is squeezed is a chart that
    /// lies — the history bars would be cut short and the project
    /// would look like it had less history than it has.
    fn name_width(&self, area_width: u16) -> u16 {
        let inner = area_width.saturating_sub(BORDERS);
        let spacing = (self.count() as u16 - 1).saturating_mul(COLUMN_SPACING);
        let available = inner.saturating_sub(spacing);
        available
            .saturating_sub(self.fixed().saturating_sub(NAME_MIN))
            .max(NAME_MIN)
    }
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
        .border_style(Style::new().fg(LINE))
        .style(Style::new().bg(PANEL_0)))
    .style(Style::new().fg(TEXT).bg(PANEL_0));

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
    // The rows are the filtered cues, but the selection is an index
    // into the whole list, so the original index is kept with each row.
    let filtered: Vec<(usize, &clavus_core::types::Cue)> = app
        .cues
        .iter()
        .enumerate()
        .filter(|(_, c)| filter.is_none() || filter.as_ref().is_some_and(|f| c.status == *f))
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

    let cue_rows: Vec<ListItem<'_>> = filtered
        .iter()
        .map(|(index, c)| {
            let color = cue_status_color(&c.status);
            let icon = cue_status_icon(&c.status);

            // The selected row is highlighted. The arrow keys moved a
            // cursor that could not be seen, so the user did not know
            // which note the keys acted on.
            //
            // The highlight is a raised surface rather than a block of
            // the full accent: a bright bar that travels as the cursor
            // moves is hard to read and draws the eye.
            let selected = app.selected_cue == Some(*index);
            let bg = if selected { ACCENT_DIM } else { PANEL_0 };
            let fg = if selected { TEXT } else { color };

            let line1 = Line::raw(format!(
                "{} {}  {:12}  {}",
                icon, c.status.as_str(), c.position, c.text,
            ))
            .style(Style::new().fg(fg).bg(bg));

            let line2 = Line::raw(format!(
                "      {} · {} · {}",
                c.track_name,
                clavus_core::short(&c.id, 8),
                c.author
            ))
                .style(Style::new().fg(MUTED).bg(bg));

            ListItem::new(vec![line1, line2])
        })
        .collect();

    let list = List::new(cue_rows)
        .block(Block::default()
            .borders(Borders::ALL)
            .border_style(Style::new().fg(LINE))
            .style(Style::new().bg(PANEL_0)))
        .style(Style::new().fg(TEXT).bg(PANEL_0));

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

    // The form asks for the message first and the position second, so
    // the cue is stored the same way the CLI stores it: the message in
    // the text, the position in the position.
    let typed = app.input_buf.trim();
    let typed_display = if typed.is_empty() { "…" } else { typed };

    let (message, position) = match app.cue_step {
        crate::app::CueStep::Message => (typed_display.to_string(), "1.1.1".to_string()),
        crate::app::CueStep::Position => (
            if app.cue_message.trim().is_empty() {
                "…".to_string()
            } else {
                app.cue_message.trim().to_string()
            },
            if typed.is_empty() {
                // The default is shown, but it is not typed into the
                // field, so the user can type their own position
                // without having to delete it first.
                format!("{} (default)", crate::app::DEFAULT_CUE_POSITION)
            } else {
                typed_display.to_string()
            },
        ),
    };

    let message_style = match app.cue_step {
        crate::app::CueStep::Message => Style::new().fg(ACCENT),
        _ => Style::new().fg(TEXT),
    };
    let position_style = match app.cue_step {
        crate::app::CueStep::Position => Style::new().fg(ACCENT),
        _ => Style::new().fg(TEXT),
    };

    let form_lines = vec![
        l("", MUTED),
        l("  MESSAGE  (what should be noted here?)", MUTED),
        l("", MUTED),
        Line::raw(format!("  {}█", message)).style(message_style),
        l("", MUTED),
        l("  POSITION  (bars.beats.sixteenths)", MUTED),
        l("", MUTED),
        Line::raw(format!("  {}█", position)).style(position_style),
        l("", MUTED),
        l("  TRACK", MUTED),
        l("  Arrangement", TEXT),
        l("", MUTED),
        Line::raw(format!("  author: {}", clavus_core::default_author()))
            .style(Style::new().fg(MUTED)),
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

    let snap_rows: Vec<Row<'_>> = app
        .snapshots
        .iter()
        .enumerate()
        .map(|(row, s)| {
            let ts = unix_to_rel(s.timestamp as i64);
            // The selected row is highlighted — see the cue list.
            let selected = app.selected_snapshot == Some(row);
            let bg = if selected { ACCENT_DIM } else { PANEL_0 };
            let fg = TEXT;
            Row::new(vec![
                Span::raw(clavus_core::short(&s.hash, 8)).style(Style::new().fg(if selected { ACCENT } else { WARM })),
                Span::raw(s.message.replace('\n', " ")),
                Span::raw(format!("{:.0}", s.bpm)),
                Span::raw(s.track_count.to_string()),
                Span::raw(s.sample_hashes.len().to_string()),
                Span::raw(ts),
            ])
            .style(Style::new().fg(fg).bg(bg))
            .height(1)
        })
        .collect();

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
        .border_style(Style::new().fg(LINE))
        .style(Style::new().bg(PANEL_0)))
    .style(Style::new().fg(TEXT).bg(PANEL_0));

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
        Line::raw(format!("  {}", app.relay_address())).style(Style::new().fg(TEXT)),
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
        l("  P  push to relay", MUTED),
        l("  G  pull from relay", MUTED),
        l("  r  change relay address", MUTED),
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
    if area.height == 0 || area.width == 0 {
        return;
    }
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
    // The width is counted in *characters*, not bytes. The hint has
    // arrows and dots in it, and a byte count made the hint wider than
    // the window — the paragraph was then drawn past the end of the
    // buffer, and ratatui panicked. A narrow window now clips the
    // hint instead of dying.
    let hint_w = (hint.chars().count() as u16).saturating_add(2).min(area.width);
    frame.render_widget(hint_para, Rect::new(area.right().saturating_sub(hint_w), area.y, hint_w, area.height));

    // Bottom border
    let border = Paragraph::new("").style(Style::new().fg(LINE));
    frame.render_widget(
        border,
        Rect::new(
            area.x,
            area.y + area.height.saturating_sub(1),
            area.width,
            1,
        ),
    );
}

// ─── Footer / status bar ─────────────────────────────────────────────

// The bar is three regions, and the two outer ones never change size:
//
//   | what the list shows |        the message         | the relay |
//
// The message lives in the middle and is cut to fit. It can be any
// length — a long error, a short hint — and the layout does not move,
// because nothing either side of it can grow. That is the whole
// answer to a notification that shoved the rest of the bar sideways:
// the message is given a box of its own and told to stay in it.

const FOOTER_LEFT: u16 = 24;
const FOOTER_RIGHT: u16 = 12;

fn render_footer(frame: &mut Frame, area: Rect, app: &App) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    frame.render_widget(Paragraph::new("").style(Style::new().bg(PANEL_0)), area);

    let left_w = FOOTER_LEFT.min(area.width);
    let right_w = FOOTER_RIGHT.min(area.width.saturating_sub(left_w));
    let mid_w = area.width.saturating_sub(left_w + right_w);

    // ── Left: how the list is being shown ─────────────────────────
    let left_text = footer_left_text(app);
    frame.render_widget(
        Paragraph::new(Line::raw(fit(&left_text, left_w as usize)))
            .style(Style::new().fg(MUTED).bg(PANEL_0)),
        Rect::new(area.x, area.y, left_w, area.height),
    );

    // ── Middle: the message ────────────────────────────────────────
    let (message, colour) = if let Some(e) = &app.error {
        (e.clone(), RED)
    } else if let Some(i) = &app.info {
        (i.clone(), ACCENT)
    } else {
        (app.footer_hint(), MUTED)
    };
    // Cut to the region's width, with an ellipsis so it is obvious
    // that there is more.
    let shown = fit_ellipsis(&message, mid_w as usize);
    frame.render_widget(
        Paragraph::new(Line::raw(shown)).style(Style::new().fg(colour).bg(PANEL_0)),
        Rect::new(area.x + left_w, area.y, mid_w, area.height),
    );

    // ── Right: the relay ───────────────────────────────────────────
    let (dot, label, dot_colour) = if app.sync_state.connected {
        ("\u{25cf}", "relay", CYAN)
    } else {
        ("\u{25cb}", "offline", MUTED)
    };
    let right_text = format!("{} {}", dot, label);
    let padded = fit(&right_text, right_w as usize);
    // The dot keeps its colour; the rest is quiet.
    let right_spans = vec![
        Span::raw(padded.chars().next().unwrap_or(' ').to_string())
            .style(Style::new().fg(dot_colour).bg(PANEL_0)),
        Span::raw(padded.chars().skip(1).collect::<String>())
            .style(Style::new().fg(MUTED).bg(PANEL_0)),
    ];
    frame.render_widget(
        Paragraph::new(Line::from(right_spans)).style(Style::new().bg(PANEL_0)),
        Rect::new(area.x + left_w + mid_w, area.y, right_w, area.height),
    );

    // Top border
    let border = Paragraph::new("").style(Style::new().fg(LINE));
    frame.render_widget(border, Rect::new(area.x, area.y, area.width, 1));
}

/// The left region of the footer: the sort order and the filter.
///
/// While the user is typing a filter it shows what they have typed,
/// with a cursor, so it is obvious the list is being narrowed.
fn footer_left_text(app: &App) -> String {
    if app.screen != Screen::ProjectList {
        return String::new();
    }
    let mut s = format!(" {}", app.sort.label());
    if app.filtering {
        s.push_str(&format!("  /{}\u{2588}", app.filter));
    } else if !app.filter.is_empty() {
        s.push_str(&format!("  /{}", app.filter));
    }
    s
}

/// A string of exactly `width` columns.
///
/// Cut on a character boundary — a multi-byte character cut in half
/// would be a broken byte in the middle of the screen.
fn fit(text: &str, width: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out: String = chars.iter().take(width).collect();
    for _ in chars.len().min(width)..width {
        out.push(' ');
    }
    out
}

/// A string of exactly `width` columns, with an ellipsis when it had
/// to be cut.
fn fit_ellipsis(text: &str, width: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= width {
        return fit(text, width);
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "\u{2026}".to_string();
    }
    let mut out: String = chars.iter().take(width - 1).collect();
    out.push('\u{2026}');
    out
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

/// The current time, in seconds since the epoch.
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// How long ago something happened, in a short form.
///
/// A project changed today is one being worked on right now, so it is
/// warm. Everything older is quiet. The words are short on purpose:
/// the column is five columns wide.
fn age_text(secs: i64, now: i64) -> (String, Color) {
    let age = now.saturating_sub(secs).max(0);
    let (text, hot) = if age < 60 {
        ("now".to_string(), true)
    } else if age < 60 * 60 {
        (format!("{}m", age / 60), true)
    } else if age < 60 * 60 * 24 {
        (format!("{}h", age / 3600), true)
    } else if age < 60 * 60 * 24 * 7 {
        (format!("{}d", age / 86400), false)
    } else if age < 60 * 60 * 24 * 30 {
        (format!("{}w", age / (86400 * 7)), false)
    } else if age < 60 * 60 * 24 * 365 {
        (format!("{}mo", age / (86400 * 30)), false)
    } else {
        (format!("{}y", age / (86400 * 365)), false)
    };
    let colour = if hot { WARM } else { MUTED };
    (text, colour)
}

/// "s" or nothing.
fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// Take the top `n` rows off an area and leave the rest.
fn split_off(area: Rect, n: u16) -> [Rect; 2] {
    let n = n.min(area.height);
    [
        Rect::new(area.x, area.y, area.width, n),
        Rect::new(
            area.x,
            area.y + n,
            area.width,
            area.height.saturating_sub(n),
        ),
    ]
}

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
    if h == 0 {
        // A zero-height area used to compute `area.y + h - 1`, which
        // underflowed and then tripped ratatui.
        let empty = Rect::new(area.x, area.y, area.width, 0);
        return [empty, empty, empty];
    }
    if h == 1 {
        // One line fits the header; there is no room for the body or
        // the footer.
        return [
            Rect::new(area.x, area.y, area.width, 1),
            Rect::new(area.x, area.y, area.width, 0),
            Rect::new(area.x, area.y, area.width, 0),
        ];
    }
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
            Screen::ProjectList  => "↑↓ select  ·  Enter open  ·  / filter  ·  S sort  ·  s sync  ·  q quit",
            Screen::ProjectDetail=> "c snap  ·  l cues  ·  n snapshots  ·  s sync  ·  Esc back",
            Screen::CueList      => "↑↓ navigate  ·  a add  ·  r resolve  ·  f filter  ·  Esc back",
            Screen::CueAdd       => "Enter next/save  ·  Esc cancel",
            Screen::SnapshotList => "↑↓ navigate  ·  Esc back",
            Screen::Sync         => "P push  ·  G pull  ·  r relay  ·  Esc back",
            Screen::RelayInput   => "Enter connect  ·  Esc cancel",
        }.to_string()
    }

    pub fn footer_hint(&self) -> String {
        match self.screen {
            Screen::ProjectList  => "↑↓ navigate  ·  Enter open  ·  / filter  ·  S sort  ·  q quit",
            Screen::ProjectDetail=> "c snapshot  ·  l cues  ·  Esc",
            Screen::CueList      => "↑↓ navigate  ·  a add  ·  Esc",
            Screen::CueAdd       => "Enter next/save  ·  Esc cancel",
            Screen::SnapshotList => "↑↓ navigate  ·  Esc",
            Screen::Sync         => "P push  ·  G pull  ·  r relay  ·  Esc",
            Screen::RelayInput   => "Enter connect  ·  Esc cancel",
        }.to_string()
    }
}

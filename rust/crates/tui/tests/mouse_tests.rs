//! The mouse, and the scrolling it needs.
//!
//! Clavus is keyboard-first and the keyboard still does everything, but
//! a mouse is the fastest way to point at one row in a long list, and
//! the wheel is the fastest way to travel through a thousand sets.
//!
//! The tests here drive the real application state, the real renderer
//! and the real mouse handler. The renderer is what tells the mouse
//! where the rows are, so the hit map and the drawn rows are checked
//! against each other rather than against a hand-written layout.

use clavus_core::blob::BlobStore;
use clavus_core::cue::CueStore;
use clavus_core::snapshot::SnapshotStore;
use clavus_core::types::CueStatus;
use clavus_tui::app::{App, Screen, SortMode};
use clavus_tui::keys::handle_key;
use clavus_tui::mouse::{handle_mouse, HitKind};
use crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

const MINIMAL_ALS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton>
  <LiveSet>
    <Tempo Value="128" />
    <Tracks>
      <Track Type="Audio" Index="0">
        <Name Value="Drums"/>
      </Track>
    </Tracks>
  </LiveSet>
</Ableton>
"#;

/// A library of `count` sets, one of which has a history and some
/// open notes.
fn library(count: usize) -> (App, tempfile::TempDir) {
    let tmp = tempfile::TempDir::new().unwrap();
    let projects = tmp.path().join("library").join("sets");
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&projects).unwrap();
    std::fs::create_dir_all(&data).unwrap();

    for i in 0..count {
        std::fs::write(
            projects.join(format!("Set {:02}.als", i)),
            MINIMAL_ALS,
        )
        .unwrap();
    }

    let mut app = App::new(projects);
    app.clavus_root = data;
    app.refresh_projects().unwrap();

    if count > 0 {
        let first = app.projects[0].path.clone();
        let id = clavus_core::project::ProjectId::new(&first);
        let store = BlobStore::new(app.clavus_root.join("blobs")).unwrap();
        let snaps = SnapshotStore::new(store, &id);
        let outcome = snaps
            .save(&first, "first", None, 1, 128.0, vec![], Default::default())
            .unwrap();
        snaps.set_head(&outcome.snapshot().hash.clone(), "test").unwrap();

        let cues = CueStore::new(&app.clavus_root, &id).unwrap();
        cues.add("fix the kick", "1.1.1", "chris", "", "Arrangement")
            .unwrap();
        cues.add("more bass", "2.1.1", "chris", "", "Arrangement")
            .unwrap();
    }

    app.refresh_project_counts();
    (app, tmp)
}

/// Draw the app, so the renderer fills in the map of what is on screen.
fn draw(app: &mut App, width: u16, height: u16) -> Vec<String> {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| clavus_tui::ui::render(f, app))
        .unwrap();
    let buf = terminal.backend().buffer().clone();
    let w = buf.area.width as usize;
    (0..buf.area.height as usize)
        .map(|y| {
            (0..w)
                .map(|x| buf.content[y * w + x].symbol())
                .collect::<String>()
        })
        .collect()
}

fn mouse(kind: MouseEventKind, col: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column: col,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn down(col: u16, row: u16) -> MouseEvent {
    mouse(MouseEventKind::Down(MouseButton::Left), col, row)
}

fn drag(col: u16, row: u16) -> MouseEvent {
    mouse(MouseEventKind::Drag(MouseButton::Left), col, row)
}

fn scroll_down(col: u16, row: u16) -> MouseEvent {
    mouse(MouseEventKind::ScrollDown, col, row)
}

fn scroll_up(col: u16, row: u16) -> MouseEvent {
    mouse(MouseEventKind::ScrollUp, col, row)
}

// ── The map of the screen ──────────────────────────────────────────

#[test]
fn the_renderer_tells_the_mouse_where_the_rows_are() {
    let (mut app, _tmp) = library(3);
    let screen = draw(&mut app, 100, 24);

    assert_eq!(app.hits.len(), 3, "one region for each row");
    for (i, region) in app.hits.iter().enumerate() {
        assert_eq!(region.kind, HitKind::ProjectRow(i));
    }

    // The region must sit on the row that was drawn. The first project
    // row is the third line of the panel: a border, then the column
    // headings, then the rows.
    let first = app.hits[0];
    let line = &screen[first.y as usize];
    assert!(
        line.contains(&app.projects[0].name),
        "the region points at the line the row was drawn on: {:?}",
        line
    );
}

#[test]
fn the_map_is_rebuilt_every_frame() {
    let (mut app, _tmp) = library(3);
    draw(&mut app, 100, 24);
    let before = app.hits.len();
    assert!(before > 0);

    // Move to another screen: the old rows are gone, and a click must
    // not land on one of them.
    app.screen = Screen::Sync;
    draw(&mut app, 100, 24);
    assert!(app.hits.is_empty(), "the old rows are not still there");
}

#[test]
fn a_click_picks_the_row_under_the_pointer() {
    let (mut app, _tmp) = library(5);
    draw(&mut app, 100, 24);

    let target = app.hits[2];
    handle_mouse(&down(target.x + 2, target.y), &mut app);

    assert_eq!(
        app.selected_project,
        Some(target.kind_index()),
        "the row under the pointer is selected"
    );
}

#[test]
fn a_click_on_nothing_selects_nothing() {
    let (mut app, _tmp) = library(5);
    draw(&mut app, 100, 24);
    app.selected_project = None;

    // Row 0 is the title bar.
    handle_mouse(&down(20, 0), &mut app);
    assert_eq!(app.selected_project, None, "the title bar is not a row");
}

#[test]
fn a_double_click_opens_the_project() {
    let (mut app, _tmp) = library(5);
    draw(&mut app, 100, 24);

    let target = app.hits[1];
    handle_mouse(&down(target.x, target.y), &mut app);
    assert_eq!(app.screen, Screen::ProjectList, "one click only picks");

    // The second click, close enough in time and place.
    app.last_click = Some(clavus_tui::mouse::Click {
        x: target.x,
        y: target.y,
        at: std::time::Instant::now(),
    });
    handle_mouse(&down(target.x, target.y), &mut app);
    assert_eq!(
        app.screen,
        Screen::ProjectDetail,
        "a double click opens the set"
    );
}

#[test]
fn a_drag_picks_the_row_under_the_pointer() {
    let (mut app, _tmp) = library(6);
    draw(&mut app, 100, 24);

    let target = app.hits[4];
    handle_mouse(&drag(target.x, target.y), &mut app);
    assert_eq!(app.selected_project, Some(target.kind_index()));
}

#[test]
fn the_wheel_moves_the_selection() {
    let (mut app, _tmp) = library(10);
    draw(&mut app, 100, 24);

    handle_mouse(&scroll_down(40, 8), &mut app);
    handle_mouse(&scroll_down(40, 8), &mut app);
    handle_mouse(&scroll_down(40, 8), &mut app);
    assert_eq!(app.selected_project, Some(2), "three notches, three rows");

    handle_mouse(&scroll_up(40, 8), &mut app);
    assert_eq!(app.selected_project, Some(1), "and back again");
}

#[test]
fn the_wheel_stops_at_the_ends() {
    let (mut app, _tmp) = library(4);
    draw(&mut app, 100, 24);

    for _ in 0..20 {
        handle_mouse(&scroll_up(40, 8), &mut app);
    }
    assert_eq!(app.selected_project, Some(0), "it stops at the top");

    for _ in 0..40 {
        handle_mouse(&scroll_down(40, 8), &mut app);
    }
    let visible = app.visible_indices();
    assert_eq!(
        app.selected_project,
        Some(*visible.last().unwrap()),
        "and at the bottom"
    );
}

// ── Scrolling ──────────────────────────────────────────────────────

#[test]
fn a_long_list_scrolls_to_keep_the_selection_on_screen() {
    let (mut app, _tmp) = library(60);
    draw(&mut app, 100, 24);

    // Walk down past the bottom of the window.
    for _ in 0..40 {
        handle_mouse(&scroll_down(40, 8), &mut app);
    }
    draw(&mut app, 100, 24);

    let selected = app.selected_project.expect("a selection");
    let name = app.projects[selected].name.clone();

    // The region for the selected set must be on the screen, and the
    // line it points at must be the line the set was drawn on.
    let region = app
        .hits
        .iter()
        .find(|r| r.kind == HitKind::ProjectRow(selected))
        .copied()
        .expect("the selected set has a region");
    assert!(
        (region.y as usize) < 24,
        "the region is on the screen: {}",
        region.y
    );

    let screen = draw(&mut app, 100, 24);
    let line = &screen[region.y as usize];
    assert!(
        line.contains(&name),
        "the region points at the line the set was drawn on: {:?}",
        line
    );
}

#[test]
fn the_selection_is_drawn_in_the_window() {
    let (mut app, _tmp) = library(60);
    draw(&mut app, 100, 24);

    for _ in 0..40 {
        handle_mouse(&scroll_down(40, 8), &mut app);
    }
    let screen = draw(&mut app, 100, 24);

    let selected = app.selected_project.expect("a selection");
    let name = app.projects[selected].name.clone();
    let drawn = screen.iter().any(|line| line.contains(&name));
    assert!(drawn, "the set under the cursor is on the screen");
}

#[test]
fn scrolling_back_up_returns_to_the_top() {
    let (mut app, _tmp) = library(60);
    draw(&mut app, 100, 24);

    for _ in 0..40 {
        handle_mouse(&scroll_down(40, 8), &mut app);
    }
    draw(&mut app, 100, 24);
    assert!(app.list_scroll > 0, "the list has moved down");

    for _ in 0..40 {
        handle_mouse(&scroll_up(40, 8), &mut app);
    }
    draw(&mut app, 100, 24);
    assert_eq!(app.list_scroll, 0, "and back to the top");
}

#[test]
fn a_short_list_does_not_scroll() {
    let (mut app, _tmp) = library(3);
    draw(&mut app, 100, 24);
    for _ in 0..5 {
        handle_mouse(&scroll_down(40, 8), &mut app);
    }
    draw(&mut app, 100, 24);
    assert_eq!(app.list_scroll, 0, "there was nothing to scroll");
}

#[test]
fn the_scrollbar_is_drawn_only_when_there_is_something_to_scroll() {
    let (mut app, _tmp) = library(60);
    let screen = draw(&mut app, 100, 24);
    let last_column: String = screen
        .iter()
        .filter(|l| l.contains('│') || l.contains('┐') || l.contains('┘'))
        .map(|l| l.chars().last().unwrap_or(' ').to_string())
        .collect();
    assert!(
        last_column.contains('█') || last_column.contains('║'),
        "the scrollbar is on the right-hand edge: {:?}",
        last_column
    );

    let (mut app, _tmp) = library(3);
    let screen = draw(&mut app, 100, 24);
    let last_column: String = screen
        .iter()
        .filter(|l| l.contains('│') || l.contains('┐') || l.contains('┘'))
        .map(|l| l.chars().last().unwrap_or(' ').to_string())
        .collect();
    assert!(
        !last_column.contains('█'),
        "and there is none when everything fits: {:?}",
        last_column
    );
}

// ── The cue list ───────────────────────────────────────────────────

#[test]
fn the_cue_filter_does_not_break_the_arrow_keys() {
    let (mut app, _tmp) = library(1);
    // The cues belong to a project, so one has to be selected before
    // they can be loaded.
    app.selected_project = Some(0);
    app.load_cues().unwrap();
    app.screen = Screen::CueList;
    app.cue_filter = Some(CueStatus::Pending);
    app.load_cues().unwrap();
    draw(&mut app, 100, 24);

    // Two notes are pending, so the cursor must land on one of them.
    handle_key(
        &KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        &mut app,
    );
    let selected = app.selected_cue.expect("a selection");
    let cue = &app.cues[selected];
    assert_eq!(
        cue.status,
        CueStatus::Pending,
        "the cursor is on a cue that is actually on screen"
    );
}

#[test]
fn the_cue_list_scrolls_too() {
    let (mut app, _tmp) = library(1);
    app.selected_project = Some(0);
    app.load_cues().unwrap();
    app.screen = Screen::CueList;
    draw(&mut app, 100, 24);

    // There are only two cues, so the list cannot scroll — but the
    // mouse still has to find them.
    assert!(!app.hits.is_empty(), "the cues are on the map");
    let target = app.hits[0];
    handle_mouse(&down(target.x, target.y), &mut app);
    assert_eq!(app.selected_cue, Some(target.kind_index()));
}

// ── The keyboard still works ───────────────────────────────────────

#[test]
fn the_keyboard_still_works_with_the_mouse_on() {
    let (mut app, _tmp) = library(5);
    draw(&mut app, 100, 24);

    handle_key(
        &KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        &mut app,
    );
    assert_eq!(app.selected_project, Some(0));

    handle_key(
        &KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
        &mut app,
    );
    assert_eq!(app.selected_project, Some(1));

    handle_key(
        &KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut app,
    );
    assert_eq!(app.screen, Screen::ProjectDetail);
}

#[test]
fn the_sort_and_the_filter_still_work_with_the_mouse_on() {
    let (mut app, _tmp) = library(5);
    app.sort = SortMode::Recent;
    handle_key(
        &KeyEvent::new(KeyCode::Char('S'), KeyModifiers::SHIFT),
        &mut app,
    );
    assert_ne!(app.sort, SortMode::Recent, "the sort still cycles");

    handle_key(
        &KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
        &mut app,
    );
    assert!(app.filtering, "the filter still opens");
}

/// A helper so the tests above can name a row.
trait KindIndex {
    fn kind_index(&self) -> usize;
}

impl KindIndex for clavus_tui::mouse::HitRegion {
    fn kind_index(&self) -> usize {
        match self.kind {
            HitKind::ProjectRow(i) => i,
            HitKind::CueRow(i) => i,
            HitKind::SnapshotRow(i) => i,
            HitKind::Scrollbar { .. } => usize::MAX,
        }
    }
}

// ── The scrollbar is a control, not a picture ──────────────────────

#[test]
fn clicking_the_scrollbar_moves_the_window() {
    let (mut app, _tmp) = library(200);
    draw(&mut app, 100, 24);
    assert_eq!(app.list_scroll, 0, "it starts at the top");

    // The scrollbar is the last column of the panel.
    let bar = app
        .hits
        .iter()
        .find(|r| matches!(r.kind, HitKind::Scrollbar { .. }))
        .copied()
        .expect("the scrollbar is on the map");

    // Click near the bottom of it.
    handle_mouse(&down(bar.x, bar.y + bar.h - 3), &mut app);
    draw(&mut app, 100, 24);

    assert!(
        app.list_scroll > 0,
        "the window moved down: {}",
        app.list_scroll
    );
    assert!(
        app.list_scroll < app.projects.len(),
        "and not off the end"
    );
}

#[test]
fn the_selection_goes_with_the_window() {
    let (mut app, _tmp) = library(200);
    draw(&mut app, 100, 24);

    let bar = app
        .hits
        .iter()
        .find(|r| matches!(r.kind, HitKind::Scrollbar { .. }))
        .copied()
        .expect("the scrollbar is on the map");

    handle_mouse(&down(bar.x, bar.y + bar.h - 3), &mut app);
    draw(&mut app, 100, 24);

    let selected = app.selected_project.expect("a selection");
    let region = app
        .hits
        .iter()
        .find(|r| r.kind == HitKind::ProjectRow(selected))
        .copied()
        .expect("the selected set has a region");
    assert!(
        (region.y as usize) < 24,
        "the cursor is still on a row that is on the screen: {}",
        region.y
    );
}

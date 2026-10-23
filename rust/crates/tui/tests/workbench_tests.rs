//! Browsing a library of projects.
//!
//! A user with a thousand sets needs to find one of them, and needs to
//! know which ones they are actually working on. The list grew two
//! columns for the second question — the shape of a project's history,
//! and the notes still open on it — and two ways to answer the first:
//! a filter and a sort order.
//!
//! And the whole thing sits in a footer that has three regions of
//! fixed width, so a message of any length cannot move the layout.
//! That is the point of it: a notification that shoves the rest of the
//! bar sideways is a notification the user cannot read.

use clavus_core::blob::BlobStore;
use clavus_core::cue::CueStore;
use clavus_core::snapshot::SnapshotStore;
use clavus_core::types::CueStatus;
use clavus_tui::app::{App, Screen, SortMode};
use clavus_tui::keys::handle_key;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
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

/// A library with three sets, a history, and some open notes.
fn library() -> (App, tempfile::TempDir) {
    let tmp = tempfile::TempDir::new().unwrap();
    let projects = tmp.path().join("library").join("sets");
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&projects).unwrap();
    std::fs::create_dir_all(&data).unwrap();

    for name in ["Golf", "Modal", "Zulu"] {
        std::fs::write(projects.join(format!("{}.als", name)), MINIMAL_ALS).unwrap();
    }

    let mut app = App::new(projects);
    app.clavus_root = data;
    app.refresh_projects().unwrap();

    // Give Golf some history, so it has a shape.
    let alpha = app.projects[index_of(&app, "Golf")].path.clone();
    let id = clavus_core::project::ProjectId::new(&alpha);
    let store = BlobStore::new(app.clavus_root.join("blobs")).unwrap();
    let snaps = SnapshotStore::new(store, &id);
    let outcome = snaps
        .save(&alpha, "first", None, 1, 128.0, vec![], Default::default())
        .unwrap();
    let head = outcome.snapshot().hash.clone();
    snaps.set_head(&head, "test").unwrap();

    // And two notes left open on it.
    let cues = CueStore::new(&app.clavus_root, &id).unwrap();
    cues.add("fix the kick", "1.1.1", "chris", "", "Arrangement").unwrap();
    cues.add("more bass", "2.1.1", "chris", "", "Arrangement").unwrap();

    app.refresh_project_counts();
    (app, tmp)
}

fn render(app: &mut App, width: u16, height: u16) -> Vec<String> {
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

/// The index of a project in the list, by name.
///
/// The list is ordered by recency, and three files written in the same
/// instant have no guaranteed order — so a test that says
/// `projects[0]` is asking a question the code never promised an
/// answer to.
fn index_of(app: &App, name: &str) -> usize {
    app.projects
        .iter()
        .position(|p| p.name == name)
        .unwrap_or_else(|| panic!("no project called {}", name))
}

/// Where a piece of text sits in a line, counted in *characters*.
///
/// Byte offsets lie here: the bar has arrows and dots in it, and the
/// message changes from one to the next, so the same text can sit at
/// the same column and a different byte offset.
fn column_of(line: &str, needle: &str) -> Option<usize> {
    let chars: Vec<char> = line.chars().collect();
    let n: Vec<char> = needle.chars().collect();
    chars
        .windows(n.len())
        .position(|w| w == n.as_slice())
}

fn key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

// ─── Finding a project ────────────────────────────────────────────────

#[test]
fn the_filter_narrows_the_list_to_what_was_typed() {
    let (mut app, _tmp) = library();
    app.set_filter("lf".into());
    let visible = app.visible_indices();
    assert_eq!(visible.len(), 1, "only Golf matches");
    assert_eq!(app.projects[visible[0]].name, "Golf");
}

#[test]
fn the_filter_looks_in_the_folder_too() {
    let (mut app, _tmp) = library();
    // Every project is in "library", so the folder name matches all
    // of them — and the name matches none.
    app.set_filter("library".into());
    assert_eq!(
        app.visible_indices().len(),
        3,
        "the folder is searched as well as the name"
    );
}

#[test]
fn an_empty_filter_shows_everything() {
    let (mut app, _tmp) = library();
    app.set_filter(String::new());
    assert_eq!(app.visible_indices().len(), 3);
}

#[test]
fn the_filter_does_not_move_the_selection_to_another_project() {
    let (mut app, _tmp) = library();
    let gamma = index_of(&app, "Zulu");
    let beta = index_of(&app, "Modal");
    app.selected_project = Some(gamma);
    app.set_filter("lf".into());
    app.ensure_selection_visible();
    // Zulu is not on show any more, so the selection lands on the
    // first project that is.
    assert_eq!(app.projects[app.selected_project.unwrap()].name, "Golf");

    // But narrowing to something that *does* include the selection
    // leaves the selection exactly where it was.
    app.selected_project = Some(beta);
    app.set_filter("da".into());
    app.ensure_selection_visible();
    assert_eq!(
        app.projects[app.selected_project.unwrap()].name,
        "Modal",
        "the selection stays on the project it was on"
    );
}

#[test]
fn typing_in_the_filter_does_not_quit_the_program() {
    let (mut app, _tmp) = library();
    app.screen = Screen::ProjectList;
    handle_key(&key('/'), &mut app);
    assert!(app.filtering, "the filter takes the keyboard");

    // A "q" in a search term is a letter, not the quit key.
    let quit = handle_key(&key('q'), &mut app);
    assert!(!quit, "typing must not quit");
    assert_eq!(app.filter, "q");
}

#[test]
fn escape_throws_the_filter_away() {
    let (mut app, _tmp) = library();
    app.screen = Screen::ProjectList;
    handle_key(&key('/'), &mut app);
    handle_key(&key('a'), &mut app);
    handle_key(&key('l'), &mut app);
    assert_eq!(app.filter, "al");
    handle_key(
        &KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        &mut app,
    );
    assert_eq!(app.filter, "", "escape clears the filter");
    assert!(!app.filtering);
}

// ─── Ordering the list ───────────────────────────────────────────────

#[test]
fn the_list_can_be_sorted_by_name() {
    let (mut app, _tmp) = library();
    app.sort = SortMode::Name;
    let names: Vec<&str> = app
        .visible_indices()
        .iter()
        .map(|&i| app.projects[i].name.as_str())
        .collect();
    assert_eq!(names, vec!["Golf", "Modal", "Zulu"]);
}

#[test]
fn the_list_can_be_sorted_by_the_notes_still_open() {
    let (mut app, _tmp) = library();
    // Golf has two open notes; the others have none.
    app.sort = SortMode::Open;
    let first = app.visible_indices()[0];
    assert_eq!(
        app.projects[first].name, "Golf",
        "the project with the most unfinished work comes first"
    );
}

#[test]
fn the_sort_key_changes_the_order_and_says_so() {
    let (mut app, _tmp) = library();
    app.screen = Screen::ProjectList;
    let before = app.sort;
    handle_key(&KeyEvent::new(KeyCode::Char('S'), KeyModifiers::NONE), &mut app);
    assert_ne!(app.sort, before, "the sort changed");
    assert!(app.info.is_some(), "the user is told");
}

#[test]
fn moving_the_cursor_walks_the_projects_on_show() {
    let (mut app, _tmp) = library();
    app.set_filter("o".into()); // Golf and Modal
    app.selected_project = None;
    app.move_selection(1);
    let first = app.selected_project.unwrap();
    app.move_selection(1);
    let second = app.selected_project.unwrap();
    assert_ne!(first, second, "the cursor moved");
    assert_eq!(
        app.visible_indices().len(),
        2,
        "there were two to move between"
    );
    app.move_selection(1);
    assert_eq!(
        app.selected_project.unwrap(),
        second,
        "the cursor stops at the end"
    );
}

// ─── Knowing what you are working on ─────────────────────────────────

#[test]
fn a_project_with_open_notes_says_so() {
    let (mut app, _tmp) = library();
    let alpha = app
        .projects
        .iter()
        .find(|p| p.name == "Golf")
        .unwrap();
    assert_eq!(alpha.open_cues, 2, "two notes are still open");

    let rows = render(&mut app, 100, 24);
    let row = rows.iter().find(|r| r.contains("Golf")).unwrap();
    assert!(
        row.contains("\u{25cb}2"),
        "the open notes are on the row: {:?}",
        row
    );
}

#[test]
fn the_shape_of_a_projects_history_is_drawn() {
    let (mut app, _tmp) = library();
    let alpha = app.projects.iter().find(|p| p.name == "Golf").unwrap();
    assert!(
        !alpha.activity.is_empty(),
        "a project with a snapshot has a shape"
    );
    assert!(
        alpha.activity.iter().any(|&h| h > 0),
        "and the shape has something in it"
    );

    let rows = render(&mut app, 100, 24);
    let row = rows.iter().find(|r| r.contains("Golf")).unwrap();
    assert!(
        row.contains('\u{2581}') || row.contains('\u{2582}') || row.contains('\u{2588}'),
        "the history is drawn on the row: {:?}",
        row
    );
}

#[test]
fn the_strip_says_which_project_is_being_looked_at() {
    let (mut app, _tmp) = library();
    app.selected_project = Some(index_of(&app, "Golf"));
    let rows = render(&mut app, 100, 24);
    let strip = &rows[1];
    assert!(
        strip.contains("Golf"),
        "the strip names the project: {:?}",
        strip
    );
    assert!(
        strip.contains("snapshot"),
        "and says how much history it has: {:?}",
        strip
    );
}

// ─── The footer does not move ─────────────────────────────────────────

#[test]
fn a_message_of_any_length_does_not_move_the_layout() {
    let (mut app, _tmp) = library();
    app.screen = Screen::ProjectList;

    // The right-hand region is the relay's, and it must stay exactly
    // where it was.
    let quiet = render(&mut app, 100, 24);
    let quiet_x = column_of(quiet.last().unwrap(), "offline").unwrap();

    app.set_error("something went wrong and it has a great deal to say".into());
    let loud = render(&mut app, 100, 24);
    let loud_x = column_of(loud.last().unwrap(), "offline").unwrap();

    assert_eq!(
        quiet_x, loud_x,
        "the relay's place in the bar did not move:\n{}\n{}",
        quiet.last().unwrap(),
        loud.last().unwrap()
    );

    // And the message is still there — cut to fit, not dropped.
    assert!(
        loud.last().unwrap().contains("something went wrong"),
        "the message is still shown: {:?}",
        loud.last().unwrap()
    );
}

#[test]
fn a_very_long_message_is_cut_rather_than_shown() {
    let (mut app, _tmp) = library();
    app.screen = Screen::ProjectList;
    app.set_error("x".repeat(500));
    let rows = render(&mut app, 100, 24);
    let footer = rows.last().unwrap();
    assert_eq!(
        footer.chars().count(),
        100,
        "the bar is still exactly as wide as the window"
    );
    assert!(
        footer.contains('\u{2026}'),
        "and it says that there was more: {:?}",
        footer
    );
}

#[test]
fn the_bar_survives_a_narrow_window() {
    let (mut app, _tmp) = library();
    app.screen = Screen::ProjectList;
    app.set_error("a message".into());
    for width in [20u16, 30, 45, 60] {
        let rows = render(&mut app, width, 12);
        assert_eq!(
            rows.last().unwrap().chars().count(),
            width as usize,
            "the bar is the width of the window at {}",
            width
        );
    }
}

// ─── The list copes with a narrow window ──────────────────────────────

#[test]
fn the_history_is_never_squeezed() {
    // A chart that is cut short is a chart that lies: the project
    // would look like it had less history than it has.
    let (mut app, _tmp) = library();
    for width in [60u16, 62, 70, 73, 79, 86, 100, 140] {
        let rows = render(&mut app, width, 24);
        let row = rows.iter().find(|r| r.contains("Golf")).unwrap();
        let bars = row
            .chars()
            .filter(|c| "\u{2581}\u{2582}\u{2583}\u{2584}\u{2585}\u{2586}\u{2587}\u{2588}".contains(*c))
            .count();
        assert_eq!(
            bars,
            clavus_tui::app::ACTIVITY_CELLS,
            "at {} columns the history was cut to {} cells",
            width,
            bars
        );
    }
}

#[test]
fn a_narrow_window_still_draws_the_name() {
    let (mut app, _tmp) = library();
    let rows = render(&mut app, 60, 24);
    assert!(
        rows.iter().any(|r| r.contains("Golf")),
        "the project is still on screen"
    );
}

#[test]
fn the_title_bar_does_not_panic_in_a_narrow_window() {
    // The hint has arrows and dots in it. Counting its *bytes* made it
    // wider than the window, and ratatui panicked when it drew past
    // the end of the buffer.
    let (mut app, _tmp) = library();
    for width in [20u16, 30, 45, 60] {
        let rows = render(&mut app, width, 12);
        assert_eq!(rows.len(), 12);
    }
}

// ─── An honest list ───────────────────────────────────────────────────

#[test]
fn a_filter_that_matches_nothing_says_so() {
    let (mut app, _tmp) = library();
    app.set_filter("zzzz".into());
    let rows = render(&mut app, 100, 24);
    assert!(
        rows.iter().any(|r| r.contains("no match")),
        "the list says that nothing matched: {:?}",
        rows
    );
}

#[test]
fn the_notes_still_open_are_the_ones_that_were_not_closed() {
    let (mut app, _tmp) = library();
    let alpha = app.projects[index_of(&app, "Golf")].path.clone();
    let id = clavus_core::project::ProjectId::new(&alpha);
    let cues = CueStore::new(&app.clavus_root, &id).unwrap();

    // Close one of them.
    let open = cues.list(None).unwrap();
    let first = open
        .iter()
        .find(|c| c.status == CueStatus::Pending)
        .unwrap();
    cues.resolve(&first.id, "done").unwrap();

    app.refresh_project_counts();
    let alpha_entry = app.projects.iter().find(|p| p.name == "Golf").unwrap();
    assert_eq!(
        alpha_entry.open_cues, 1,
        "a resolved note is no longer open"
    );
}

// ── Capital letters, the way a terminal actually sends them ─────────
//
// crossterm reports an uppercase letter with the shift modifier set.
// Every test that built its key events by hand used no modifiers, so
// the capital-letter shortcuts were alive in the tests and dead in the
// program. These are the shapes a real terminal sends.

fn shift(c: char) -> crossterm::event::KeyEvent {
    crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char(c),
        crossterm::event::KeyModifiers::SHIFT,
    )
}

fn plain(c: char) -> crossterm::event::KeyEvent {
    crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char(c),
        crossterm::event::KeyModifiers::NONE,
    )
}

#[test]
fn the_sort_key_works_the_way_a_terminal_sends_it() {
    let (mut app, _tmp) = library();
    app.sort = SortMode::Recent;
    handle_key(&shift('S'), &mut app);
    assert_ne!(app.sort, SortMode::Recent, "a shifted S must still sort");

    // Twice more lands on the sort by history, where the one set with
    // a history comes first. The three sets are already in
    // alphabetical order, so sorting by name cannot move anything —
    // this is the order that actually differs.
    handle_key(&shift('S'), &mut app);
    assert_eq!(app.sort, SortMode::History, "the sort cycles");
    // The order the user sees is the view's order, not the order the
    // list is held in.
    let first = app
        .visible_indices()
        .first()
        .map(|&i| app.projects[i].name.clone());
    assert_eq!(
        first.as_deref(),
        Some("Golf"),
        "the set with a history is first"
    );
}

#[test]
fn a_capital_letter_can_be_typed_into_a_search() {
    let (mut app, _tmp) = library();
    app.filtering = true;
    app.filter.clear();
    for c in "Roof".chars() {
        // A real terminal sends a capital letter with the shift
        // modifier set, and a lower-case one without it.
        if c.is_uppercase() {
            handle_key(&shift(c), &mut app);
        } else {
            handle_key(&plain(c), &mut app);
        }
    }
    assert_eq!(app.filter, "Roof", "a capital letter was dropped");
}

#[test]
fn a_capital_letter_can_be_typed_into_a_cue() {
    let (mut app, _tmp) = library();
    app.screen = Screen::CueAdd;
    app.input_buf.clear();
    for c in "OK".chars() {
        handle_key(&shift(c), &mut app);
    }
    assert_eq!(app.input_buf, "OK", "a capital letter was dropped");
}

//! The colours of the interface must be Clavus's own.
//!
//! A panel that does not set a background of its own shows the
//! *terminal's* background through. The screen is then half Clavus's
//! warm black and half whatever the terminal was configured with —
//! which looks like a patchy screen, and changes when the terminal is
//! reconfigured. Over a remote session, where the terminal on the
//! other end is a different program with a different theme, that is
//! the difference between one screen and several.
//!
//! The selected row was also a full block of the accent green. A
//! bright bar that travels as the cursor moves is hard to read and
//! draws the eye; the row is raised instead.

use clavus_core::project::ProjectId;
use clavus_tui::app::{App, Screen};
use ratatui::backend::TestBackend;
use ratatui::style::Color;
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

fn harness() -> (App, tempfile::TempDir) {
    let tmp = tempfile::TempDir::new().unwrap();
    let projects = tmp.path().join("clavus").join("projects");
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&projects).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(projects.join("demo.als"), MINIMAL_ALS).unwrap();

    let mut app = App::new(projects);
    app.clavus_root = data;
    app.refresh_projects().unwrap();
    (app, tmp)
}

/// Render a screen and return every background colour used.
fn backgrounds(app: &mut App) -> Vec<Color> {
    let backend = TestBackend::new(100, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| clavus_tui::ui::render(f, app))
        .unwrap();
    let buf = terminal.backend().buffer().clone();
    let area = buf.area;
    let mut out = Vec::new();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let cell = &buf.content
                [(y - area.top()) as usize * area.width as usize + (x - area.left()) as usize];
            out.push(cell.bg);
        }
    }
    out
}

#[test]
fn no_screen_shows_the_terminals_own_background() {
    let (mut app, _tmp) = harness();

    let id = ProjectId::new(&app.projects[0].path.clone());
    let cue_store = clavus_core::CueStore::new(&app.clavus_root, &id).unwrap();
    cue_store.add("a note", "1.1.1", "someone", "", "Arrangement").unwrap();

    let screens = [
        Screen::ProjectList,
        Screen::ProjectDetail,
        Screen::CueList,
        Screen::CueAdd,
        Screen::SnapshotList,
        Screen::Sync,
        Screen::RelayInput,
    ];

    for screen in screens {
        app.screen = screen;
        let bgs = backgrounds(&mut app);
        assert!(
            !bgs.is_empty(),
            "screen {:?} rendered nothing",
            screen
        );
        if bgs.contains(&Color::Reset) {
            panic!(
                "screen {:?} leaves {} cell(s) on the terminal's own background",
                screen,
                bgs.iter().filter(|c| **c == Color::Reset).count()
            );
        }
    }
}

#[test]
fn the_selected_row_is_raised_rather_than_flooded_with_the_accent() {
    let (mut app, _tmp) = harness();
    app.screen = Screen::ProjectList;
    app.selected_project = Some(0);

    let backend = TestBackend::new(100, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| clavus_tui::ui::render(f, &mut app))
        .unwrap();
    let buf = terminal.backend().buffer().clone();
    let area = buf.area;

    // Row 3 is the first project row.
    let row = |y: u16| {
        (area.left()..area.right())
            .map(|x| {
                buf.content
                    [(y - area.top()) as usize * area.width as usize + (x - area.left()) as usize]
                    .clone()
            })
            .collect::<Vec<_>>()
    };

    let selected = row(3);
    let unselected = row(4);

    // The selected row has its own surface.
    let sel_bg = selected[2].bg;
    let other_bg = unselected[2].bg;
    assert_ne!(
        sel_bg, other_bg,
        "the selected row must sit on a different surface from the rows around it"
    );
    assert_ne!(
        sel_bg,
        Color::Rgb(116, 224, 140),
        "the selected row must not be a block of the full accent"
    );

    // And its text is readable: the foreground is not its background.
    for cell in &selected {
        if cell.symbol() != " " {
            assert_ne!(
                cell.fg, cell.bg,
                "a character on the selected row is invisible: {:?}",
                cell.symbol()
            );
        }
    }
}

#[test]
fn moving_the_cursor_does_not_move_the_text() {
    // A row that shifts sideways when it is selected reads as a
    // glitch. The marker goes in the badge's own column so nothing
    // else moves.
    let (mut app, _tmp) = harness();
    app.screen = Screen::ProjectList;

    let render = |app: &mut App| {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| clavus_tui::ui::render(f, app))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let area = buf.area;
        let width = area.width as usize;
        let top = area.top() as usize;
        let left = area.left() as usize;
        let mut out = String::new();
        for y in 3..4usize {
            for x in 0..width {
                out.push_str(buf.content[y * width + x].symbol());
            }
        }
        let _ = (top, left);
        out
    };

    app.selected_project = None;
    let before = render(&mut app);
    app.selected_project = Some(0);
    let after = render(&mut app);

    // The name starts in the same place in both. The marker takes the
    // space the badge used to have, so nothing else shifts.
    // Character positions, not byte offsets: the marker is a
    // multi-byte character and would make a byte comparison lie.
    let name_at = |s: &str| {
        let chars: Vec<char> = s.chars().collect();
        let needle: Vec<char> = "demo".chars().collect();
        chars
            .windows(needle.len())
            .position(|w| w == needle.as_slice())
            .unwrap_or_else(|| panic!("no name in {:?}", s))
    };
    assert_eq!(
        name_at(&before),
        name_at(&after),
        "the row's text moved when it was selected:\n{}\n{}",
        before,
        after
    );

    // And the marker is there.
    assert!(
        after.contains("\u{258c}ALS"),
        "the selected row is marked: {:?}",
        after
    );
}

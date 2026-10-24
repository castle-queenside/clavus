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

    // The row the project is drawn on is found rather than assumed.
    // The list gained a strip above it, and a test that hard-codes a
    // row number breaks every time the layout moves — and then it
    // fails for a reason that has nothing to do with what it is
    // checking.
    let render = |app: &mut App| -> String {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| clavus_tui::ui::render(f, app))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let width = buf.area.width as usize;
        let mut rows: Vec<String> = Vec::new();
        for y in 0..buf.area.height as usize {
            let mut line = String::new();
            for x in 0..width {
                line.push_str(buf.content[y * width + x].symbol());
            }
            rows.push(line);
        }
        rows.into_iter()
            .find(|l| l.contains("demo") && l.contains("ALS"))
            .unwrap_or_else(|| panic!("the project was not drawn"))
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

// ─── Contrast ─────────────────────────────────────────────────────────────
//
// The user said the keybinding hints and the secondary text were hard
// to read and blended into the panel behind them. They were right, and
// it was measurable: the muted grey sat at 3.05:1 against the lightest
// panel, and the border lines at 1.23:1. Both are below what the
// WCAG asks of text, and a border at 1.23:1 is not a border at all.
//
// These tests are the reason that cannot happen again.

/// The relative luminance of a colour, per WCAG 2.1.
fn luminance(c: Color) -> f64 {
    let Color::Rgb(r, g, b) = c else {
        panic!("the palette only uses true colour");
    };
    let channel = |v: u8| {
        let v = v as f64 / 255.0;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
}

/// The contrast ratio between two colours, per WCAG 2.1.
fn contrast(a: Color, b: Color) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// The lightest surface a piece of text can end up on.
const LIGHTEST_PANEL: Color = Color::Rgb(21, 24, 26);

#[test]
fn every_text_colour_can_be_read_on_every_panel() {
    use clavus_tui::theme::*;

    // The colours that carry words. Every one of them has to clear
    // 4.5:1 — the ratio WCAG asks of ordinary text — against the
    // lightest panel it can land on.
    let text_colours: [(&str, Color); 7] = [
        ("TEXT", TEXT),
        ("MUTED", MUTED),
        ("ACCENT", ACCENT),
        ("GREEN", GREEN),
        ("WARM", WARM),
        ("RED", RED),
        ("CYAN", CYAN),
    ];

    for (name, colour) in text_colours {
        let ratio = contrast(colour, LIGHTEST_PANEL);
        assert!(
            ratio >= 4.5,
            "{} is only {ratio:.2}:1 against the lightest panel — it has to be at least 4.5:1",
            name
        );
    }
}

#[test]
fn the_primary_and_secondary_text_are_still_different() {
    use clavus_tui::theme::*;

    // Brightening the muted grey must not have made it the same as
    // the primary text. There has to be a hierarchy.
    assert!(
        contrast(TEXT, MUTED) >= 2.5,
        "primary and secondary text are only {:.2}:1 apart — they have stopped being different",
        contrast(TEXT, MUTED)
    );
}

#[test]
fn a_panel_border_can_be_seen() {
    use clavus_tui::theme::*;

    // A border that blends into the panel is not a border. The panels
    // have to read as panels.
    let ratio = contrast(LINE, LIGHTEST_PANEL);
    assert!(
        ratio >= 1.8,
        "the border is only {ratio:.2}:1 against the panel — the panels do not read as panels"
    );
    assert!(
        contrast(LINE, LINE_LO) > 1.0,
        "the dim variant must still be dimmer than the visible one"
    );
}

#[test]
fn the_history_bars_go_up_in_brightness_and_start_above_the_baseline() {
    use clavus_tui::theme::*;

    // An empty bucket is drawn with the baseline glyph. A bar that
    // holds one snapshot has to be brighter than that baseline, or a
    // project with history looks emptier than one without.
    let baseline = spark_color(0);
    let mut previous = luminance(baseline);
    for height in 1..=8u8 {
        let now = luminance(spark_color(height));
        assert!(
            now > previous,
            "spark bar {height} is not brighter than the one below it"
        );
        previous = now;
    }
}

//! Regression tests for the defects the beta agents found in the TUI.
//!
//! Each test here corresponds to a finding from the AI beta wave.

use clavus_core::types::CueStatus;
use clavus_tui::app::{App, CueStep, Screen};
use clavus_tui::handle_key;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tempfile::TempDir;

/// A minimal but valid Ableton project: 128 BPM and two tracks.
const MINIMAL_ALS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton>
  <LiveSet>
    <Tempo Value="128" />
    <Tracks>
      <Track Type="Audio" Index="0">
        <Name Value="Drums"/>
      </Track>
      <Track Type="Audio" Index="1">
        <Name Value="Bass"/>
      </Track>
    </Tracks>
    <MasterTrack>
      <DeviceChain>
        <AudioEffectChain>
          <Devices/>
        </AudioEffectChain>
      </DeviceChain>
    </MasterTrack>
  </LiveSet>
</Ableton>
"#;

fn press(app: &mut App, code: KeyCode) -> bool {
    handle_key(&KeyEvent::new(code, KeyModifiers::NONE), app)
}

fn press_char(app: &mut App, c: char) -> bool {
    handle_key(&KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE), app)
}

struct Harness {
    app: App,
    _tmp: TempDir,
}

impl Harness {
    fn with_project() -> Self {
        let tmp = TempDir::new().unwrap();
        let projects = tmp.path().join("clavus").join("projects");
        let data = tmp.path().join("data");
        std::fs::create_dir_all(&projects).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(projects.join("demo.als"), MINIMAL_ALS).unwrap();

        let mut app = App::new(projects);
        app.clavus_root = data;
        app.refresh_projects().unwrap();
        Self { app, _tmp: tmp }
    }
}

// ─── A cue carries the message in the text ────────────────────────────────
//
// Regression: the TUI passed the cue's arguments in the wrong order, so
// every cue made in the TUI had an empty message and the message in the
// position field. The CLI listed them as blank rows.

#[test]
fn a_cue_typed_in_the_tui_carries_its_message() {
    let mut h = Harness::with_project();
    press(&mut h.app, KeyCode::Down);
    press(&mut h.app, KeyCode::Enter);
    press_char(&mut h.app, 'l');
    press_char(&mut h.app, 'a');

    for c in "bring the bass in".chars() {
        press_char(&mut h.app, c);
    }
    press(&mut h.app, KeyCode::Enter);
    for c in "5.1.1".chars() {
        press_char(&mut h.app, c);
    }
    press(&mut h.app, KeyCode::Enter);

    assert_eq!(h.app.cues.len(), 1, "the cue must be stored");
    assert_eq!(h.app.cues[0].text, "bring the bass in");
    assert_eq!(h.app.cues[0].position, "5.1.1");
    // The note is signed with the user's own name, the same way the
    // CLI signs it — a collaborator saw the same person as two
    // different authors.
    assert_eq!(h.app.cues[0].author, clavus_core::default_author());
}

// ─── A "q" can be typed in a text field ───────────────────────────────────
//
// Regression: the quit key was handled before the text field, so typing
// a "q" jumped back to the project list and threw the text away. No
// word containing a lowercase "q" could be typed.

#[test]
fn a_q_can_be_typed_in_the_cue_form() {
    let mut h = Harness::with_project();
    press(&mut h.app, KeyCode::Down);
    press(&mut h.app, KeyCode::Enter);
    press_char(&mut h.app, 'l');
    press_char(&mut h.app, 'a');

    for c in "queue".chars() {
        press_char(&mut h.app, c);
    }

    assert_eq!(h.app.screen, Screen::CueAdd, "the form must still be open");
    assert_eq!(h.app.input_buf, "queue", "the text must be kept");
}

// ─── A parse error is reported ────────────────────────────────────────────
//
// Regression: pressing Enter on a project that cannot be parsed did
// nothing at all — no error, no message.

#[test]
fn a_project_that_cannot_be_parsed_reports_an_error() {
    let tmp = TempDir::new().unwrap();
    let projects = tmp.path().join("clavus").join("projects");
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&projects).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(projects.join("broken.als"), b"this is not xml at all").unwrap();

    let mut app = App::new(projects);
    app.clavus_root = data;
    app.refresh_projects().unwrap();

    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Enter);

    assert!(
        app.error.is_some(),
        "the parse error must be shown to the user"
    );
    assert_eq!(app.screen, Screen::ProjectList, "the app must stay usable");
}

// ─── The project list shows the real tempo and track count ────────────────
//
// Regression: every row showed 120 BPM and 0 tracks, whatever the
// project actually was.

#[test]
fn the_project_list_shows_the_real_tempo_and_track_count() {
    let h = Harness::with_project();
    let entry = h.app.projects.first().expect("a project");
    assert_eq!(
        entry.bpm, 128.0,
        "the tempo must come from the project file"
    );
    assert!(
        entry.track_count > 0,
        "the track count must come from the project file"
    );
}

// ─── The TUI finds projects in sub-folders ────────────────────────────────
//
// Regression: the TUI listed only the top folder, so a project the CLI
// showed was invisible in the TUI.

#[test]
fn the_tui_finds_projects_in_sub_folders() {
    let tmp = TempDir::new().unwrap();
    let projects = tmp.path().join("clavus").join("projects");
    let data = tmp.path().join("data");
    std::fs::create_dir_all(projects.join("nested")).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(projects.join("Alpha.als"), MINIMAL_ALS).unwrap();
    std::fs::write(projects.join("nested").join("Deep.als"), MINIMAL_ALS).unwrap();

    let mut app = App::new(projects);
    app.clavus_root = data;
    app.refresh_projects().unwrap();

    assert_eq!(
        app.projects.len(),
        2,
        "both projects must be found: {:?}",
        app.projects
    );
}

#[test]
fn the_tui_does_not_list_live_s_own_backup_copies() {
    // The CLI skips Live's own `Backup/` and `Autosave/` copies, and the
    // TUI has to skip them too — otherwise the two disagree about the
    // same drive, which is exactly the bug the CLI's recursive scan was.
    let tmp = TempDir::new().unwrap();
    let projects = tmp.path().join("clavus").join("projects");
    let data = tmp.path().join("data");
    let set = projects.join("MySet").join("MySet Project");
    std::fs::create_dir_all(set.join("Backup")).unwrap();
    std::fs::create_dir_all(set.join("Autosave")).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(set.join("MySet.als"), MINIMAL_ALS).unwrap();
    std::fs::write(
        set.join("Backup").join("MySet [2020-01-30 022324].als"),
        MINIMAL_ALS,
    )
    .unwrap();
    std::fs::write(set.join("Autosave").join("MySet.als"), MINIMAL_ALS).unwrap();

    let mut app = App::new(projects);
    app.clavus_root = data;
    app.refresh_projects().unwrap();

    assert_eq!(
        app.projects.len(),
        1,
        "one set, one entry — Live's copies are not projects: {:?}",
        app.projects
    );
}

// ─── The relay address can be changed ─────────────────────────────────────
//
// Regression: the TUI could only ever talk to 127.0.0.1:4477, so a
// collaborator on another machine could not use it at all.

#[test]
fn a_relay_address_can_be_typed() {
    let mut h = Harness::with_project();
    h.app.screen = Screen::Sync;

    press_char(&mut h.app, 'r');
    assert_eq!(h.app.screen, Screen::RelayInput, "the address form opens");

    // Clear the field and type a new address.
    for _ in 0..20 {
        press(&mut h.app, KeyCode::Backspace);
    }
    for c in "192.168.1.10:4477".chars() {
        press_char(&mut h.app, c);
    }
    press(&mut h.app, KeyCode::Enter);

    assert_eq!(h.app.relay_address(), "192.168.1.10:4477");
}

// ─── Every cue status can be filtered on ──────────────────────────────────
//
// Regression: the filter stopped after "skipped", so deferred and
// archived cues were invisible.

#[test]
fn every_cue_status_can_be_filtered_on() {
    let mut h = Harness::with_project();
    h.app.screen = Screen::CueList;

    let mut seen = Vec::new();
    for _ in 0..6 {
        press_char(&mut h.app, 'f');
        seen.push(h.app.cue_filter.clone());
    }
    assert!(seen.contains(&Some(CueStatus::Deferred)), "{:?}", seen);
    assert!(seen.contains(&Some(CueStatus::Archived)), "{:?}", seen);
    assert!(seen.contains(&None), "the filter must come back round: {:?}", seen);
}

// ─── Esc walks back one level and drops the text ──────────────────────────

#[test]
fn esc_walks_back_one_level_and_drops_the_text() {
    let mut h = Harness::with_project();
    press(&mut h.app, KeyCode::Down);
    press(&mut h.app, KeyCode::Enter);
    press_char(&mut h.app, 'l');
    press_char(&mut h.app, 'a');
    press_char(&mut h.app, 'x');

    press(&mut h.app, KeyCode::Esc);
    assert_eq!(h.app.screen, Screen::CueList, "Esc goes back one level");
    assert!(
        h.app.input_buf.is_empty(),
        "the abandoned text must be dropped"
    );
}

// ─── The idle phrases rotate ──────────────────────────────────────────────
//
// Regression: the index was never advanced, so the same phrase was
// shown forever and the other four were unreachable.

#[test]
fn the_idle_phrases_rotate() {
    let mut h = Harness::with_project();
    let first = h.app.idle_index;
    for _ in 0..5 {
        h.app.tick();
    }
    assert_ne!(h.app.idle_index, first, "the idle phrase must change");
}

// ─── The cue form asks for the message, then the position ────────────────

#[test]
fn the_cue_form_asks_for_the_message_first() {
    let mut h = Harness::with_project();
    press(&mut h.app, KeyCode::Down);
    press(&mut h.app, KeyCode::Enter);
    press_char(&mut h.app, 'l');
    press_char(&mut h.app, 'a');

    assert_eq!(h.app.cue_step, CueStep::Message);
    press_char(&mut h.app, 'h');
    press(&mut h.app, KeyCode::Enter);
    assert_eq!(h.app.cue_step, CueStep::Position);
    assert_eq!(h.app.cue_message, "h");
}

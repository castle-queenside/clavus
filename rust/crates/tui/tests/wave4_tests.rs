//! Regression tests for the fourth beta wave's TUI findings.
//!
//! Each test here reproduces a defect the wave-four TUI navigator
//! found against the compiled binary. They drive the real application
//! state and the real key handler — nothing is re-implemented on the
//! test side.

use clavus_core::blob::BlobStore;
use clavus_core::project::ProjectId;
use clavus_tui::app::{App, Screen};
use clavus_tui::handle_key;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tempfile::TempDir;

/// A minimal but valid Ableton project.
const MINIMAL_ALS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton>
  <LiveSet>
    <Tempo Value="128" />
    <Tracks>
      <Track Type="Audio" Index="0">
        <Name Value="Drums"/>
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

// ─── B1: two projects outside the home keep two identities ────────────
//
// The wave-four TUI navigator found this: two copies of a set in
// `external/p1/studio` and `external/p2/studio` had one relay
// identity, so the relay served one the other's history and notes.

#[test]
fn two_projects_outside_the_home_have_two_identities() {
    let home = std::path::Path::new("/Users/someone");
    let one = clavus_core::project::folder_path_of(
        std::path::Path::new("/Volumes/Studio/external/p1/studio/Song.als"),
        Some(home),
    );
    let two = clavus_core::project::folder_path_of(
        std::path::Path::new("/Volumes/Studio/external/p2/studio/Song.als"),
        Some(home),
    );
    assert_ne!(
        one, two,
        "two projects in different places must have two identities"
    );
}

// ─── B2: a pull must not move the HEAD onto another branch ────────────
//
// The TUI replaced the HEAD with the relay's branch after a pull, so
// the user's own snapshot dropped out of the chain. The CLI kept it.

#[test]
fn a_tui_pull_keeps_the_local_head_when_the_relay_offers_another_branch() {
    use clavus_core::blob::BlobStore;
    use clavus_core::snapshot::SnapshotStore;

    let h = Harness::with_project();
    let project = h.app.projects[0].path.clone();

    // The local project: one snapshot of its own.
    let id = ProjectId::new(&project);
    let store = BlobStore::new(h.app.clavus_root.join("blobs")).unwrap();
    let snap_store = SnapshotStore::new(store, &id);
    let outcome = snap_store
        .save(&project, "mine", None, 1, 128.0, vec![], Default::default())
        .expect("the local snapshot must be taken");
    let mine = outcome.snapshot().hash.clone();
    let _ = snap_store.set_head(&mine, "test");

    // The relay's branch: a different project, two snapshots long.
    let other = clavus_core::types::Snapshot {
        hash: "a".repeat(64),
        parent: None,
        timestamp: 1.0,
        message: "theirs root".into(),
        track_count: 1,
        bpm: 120.0,
        tags: vec![],
        notes: String::new(),
        sample_hashes: vec![],
        sample_paths: Default::default(),
        project_path: project.to_string_lossy().to_string(),
        project_folder: id.folder_path().to_string(),
    };
    let child = clavus_core::types::Snapshot {
        hash: "b".repeat(64),
        parent: Some(other.hash.clone()),
        ..other.clone()
    };

    let (stored, _) = clavus_tui::app::store_pulled(&[other, child], &[], &h.app.clavus_root, &id);
    assert_eq!(stored, 2, "both snapshots must be stored");

    let head = snap_store.head().expect("the HEAD must still be there");
    assert_eq!(
        head.hash, mine,
        "the user's own snapshot must stay the HEAD"
    );
}

// ─── B3: a window that is too small must not kill the app ─────────────
//
// The TUI panicked on a resize to a tiny window, and the panic hook
// swallowed the message, so the app died with a bare exit code.

#[test]
fn a_tiny_window_does_not_panic() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let mut h = Harness::with_project();
    for (w, hgt) in [(1u16, 1u16), (2, 2), (3, 10), (10, 3), (24, 6)] {
        let backend = TestBackend::new(w, hgt);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| clavus_tui::ui::render(f, &mut h.app))
            .unwrap_or_else(|e| panic!("rendering at {}x{} failed: {}", w, hgt, e));
    }
}

// ─── B5: the selected row is visible ───────────────────────────────────
//
// The arrow keys moved a cursor that was not drawn, so the user could
// not see which note the keys acted on.

#[test]
fn the_selected_cue_and_snapshot_are_highlighted() {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    let mut h = Harness::with_project();

    // A cue to select.
    let id = ProjectId::new(&h.app.projects[0].path.clone());
    let cue_store = clavus_core::CueStore::new(&h.app.clavus_root, &id).unwrap();
    cue_store
        .add("a note", "1.1.1", "someone", "", "Arrangement")
        .unwrap();

    press(&mut h.app, KeyCode::Down);
    press(&mut h.app, KeyCode::Enter);
    press_char(&mut h.app, 'l');

    let render = |app: &mut App| {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| clavus_tui::ui::render(f, app)).unwrap();
        terminal.backend().buffer().content().to_vec()
    };

    let before = render(&mut h.app);
    press(&mut h.app, KeyCode::Down);
    let after = render(&mut h.app);
    assert_ne!(
        before, after,
        "the selected cue must be visible on the screen"
    );
}

// ─── B6: a note made in the TUI is signed with the user's name ────────
//
// The TUI hard-coded "tui" as the author, so a collaborator saw the
// same person as two different authors.

#[test]
fn a_tui_cue_is_signed_with_the_users_name() {
    let mut h = Harness::with_project();
    press(&mut h.app, KeyCode::Down);
    press(&mut h.app, KeyCode::Enter);
    press_char(&mut h.app, 'l');
    press_char(&mut h.app, 'a');
    for c in "hello".chars() {
        press_char(&mut h.app, c);
    }
    press(&mut h.app, KeyCode::Enter);
    press(&mut h.app, KeyCode::Enter);

    let id = ProjectId::new(&h.app.projects[0].path.clone());
    let store = clavus_core::CueStore::new(&h.app.clavus_root, &id).unwrap();
    let cues = store.list(None).unwrap();
    assert_eq!(cues.len(), 1, "the note must be saved");
    assert_ne!(cues[0].author, "tui", "the note must not be signed 'tui'");
    assert_eq!(
        cues[0].author,
        clavus_core::default_author(),
        "the note must be signed the same way the CLI signs it"
    );
}

// ─── B7: an unreadable data directory is an error, not an empty history ──

#[test]
fn an_unreadable_data_directory_is_reported() {
    use std::os::unix::fs::PermissionsExt;

    let mut h = Harness::with_project();
    let id = ProjectId::new(&h.app.projects[0].path.clone());
    let store = BlobStore::new(h.app.clavus_root.join("blobs")).unwrap();
    let snap_store = clavus_core::snapshot::SnapshotStore::new(store, &id);
    let _ = snap_store.save(
        &h.app.projects[0].path.clone(),
        "there",
        None,
        1,
        128.0,
        vec![],
        Default::default(),
    );

    // Make the data directory unreadable.
    let mut perms = std::fs::metadata(&h.app.clavus_root).unwrap().permissions();
    perms.set_mode(0o000);
    std::fs::set_permissions(&h.app.clavus_root, perms).unwrap();

    let result = h.app.refresh_projects();
    // Put the permissions back, so the temp dir can be cleaned up.
    let mut perms = std::fs::metadata(&h.app.clavus_root).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&h.app.clavus_root, perms).unwrap();

    let err = result.expect_err("an unreadable store must be an error");
    assert!(
        err.to_lowercase().contains("cannot read"),
        "the error must say the store cannot be read: {}",
        err
    );
}

// ─── B8: the project list is refreshed when the user moves through it ──

#[test]
fn the_project_list_is_refreshed_after_an_external_snapshot() {
    let mut h = Harness::with_project();
    assert_eq!(h.app.projects[0].snap_count, 0, "there is no history yet");

    // A snapshot taken elsewhere, in the CLI.
    let id = ProjectId::new(&h.app.projects[0].path.clone());
    let store = clavus_core::blob::BlobStore::new(h.app.clavus_root.join("blobs")).unwrap();
    let snap_store = clavus_core::snapshot::SnapshotStore::new(store, &id);
    let _ = snap_store.save(
        &h.app.projects[0].path.clone(),
        "taken elsewhere",
        None,
        1,
        128.0,
        vec![],
        Default::default(),
    );

    press(&mut h.app, KeyCode::Down);
    assert_eq!(
        h.app.projects[0].snap_count, 1,
        "the list must show the snapshot that was taken elsewhere"
    );
}

// ─── B11: Esc from the sync screen goes back where it came from ────────

#[test]
fn esc_from_the_sync_screen_returns_to_the_project_list() {
    let mut h = Harness::with_project();
    press_char(&mut h.app, 's');
    assert_eq!(h.app.screen, Screen::Sync);
    press(&mut h.app, KeyCode::Esc);
    assert_eq!(
        h.app.screen,
        Screen::ProjectList,
        "Esc must go back to the screen the user came from"
    );
}

#[test]
fn esc_from_the_sync_screen_of_a_project_returns_to_the_project() {
    let mut h = Harness::with_project();
    press(&mut h.app, KeyCode::Down);
    press(&mut h.app, KeyCode::Enter);
    press_char(&mut h.app, 's');
    assert_eq!(h.app.screen, Screen::Sync);
    press(&mut h.app, KeyCode::Esc);
    assert_eq!(h.app.screen, Screen::ProjectDetail);
}

// ─── B12: typing a relay address replaces the old one ─────────────────

#[test]
fn the_relay_field_starts_empty() {
    let mut h = Harness::with_project();
    press_char(&mut h.app, 's');
    press_char(&mut h.app, 'r');
    assert_eq!(h.app.screen, Screen::RelayInput);
    assert!(
        h.app.input_buf.is_empty(),
        "the field must start empty — typing appended to the old address"
    );

    for c in "192.168.1.5:4477".chars() {
        press_char(&mut h.app, c);
    }
    assert_eq!(h.app.input_buf, "192.168.1.5:4477");
}

// ─── B13: two projects with the same name are told apart ───────────────

#[test]
fn the_project_list_shows_the_folder_of_each_project() {
    let tmp = TempDir::new().unwrap();
    let projects = tmp.path().join("clavus").join("projects");
    let data = tmp.path().join("data");
    std::fs::create_dir_all(projects.join("p1")).unwrap();
    std::fs::create_dir_all(projects.join("p2")).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(projects.join("p1").join("Song.als"), MINIMAL_ALS).unwrap();
    std::fs::write(projects.join("p2").join("Song.als"), MINIMAL_ALS).unwrap();

    let mut app = App::new(projects);
    app.clavus_root = data;
    app.refresh_projects().unwrap();
    assert_eq!(app.projects.len(), 2, "both projects must be found");

    let folders: Vec<String> = app.projects.iter().map(|p| p.folder.clone()).collect();
    assert!(
        folders[0] != folders[1],
        "the two projects must be told apart by their folder: {:?}",
        folders
    );
}

// ─── B14: a directory named like a project is not a project ────────────

#[test]
fn a_directory_named_like_a_project_is_not_listed() {
    let tmp = TempDir::new().unwrap();
    let projects = tmp.path().join("clavus").join("projects");
    let data = tmp.path().join("data");
    std::fs::create_dir_all(projects.join("Folder.als")).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(projects.join("Folder.als").join("inner.als"), MINIMAL_ALS).unwrap();
    std::fs::write(projects.join("Real.als"), MINIMAL_ALS).unwrap();

    let mut app = App::new(projects);
    app.clavus_root = data;
    app.refresh_projects().unwrap();

    let names: Vec<String> = app.projects.iter().map(|p| p.name.clone()).collect();
    assert!(
        !names.contains(&"Folder".to_string()),
        "a directory is not a project: {:?}",
        names
    );
    assert!(names.contains(&"Real".to_string()));
}

// ─── B9: notes can be pushed without a snapshot ────────────────────────
//
// The CLI sends the notes on their own. The TUI refused to send
// anything, so a cue-only user could share nothing.

#[test]
fn the_tui_pushes_notes_when_there_is_no_snapshot() {
    let mut h = Harness::with_project();
    let id = ProjectId::new(&h.app.projects[0].path.clone());
    let store = clavus_core::CueStore::new(&h.app.clavus_root, &id).unwrap();
    store
        .add(
            "a note worth sharing",
            "1.1.1",
            "someone",
            "",
            "Arrangement",
        )
        .unwrap();

    press(&mut h.app, KeyCode::Down);
    press(&mut h.app, KeyCode::Enter);
    press_char(&mut h.app, 's');

    // The push must be refused only because there is no connection —
    // not because there are no snapshots.
    assert!(!h.app.sync_state.is_connected());
    press_char(&mut h.app, 'P');
    let info = h.app.info.clone().unwrap_or_default();
    assert!(
        !info.contains("no local snapshots"),
        "the notes must not be refused for want of a snapshot: {}",
        info
    );
}

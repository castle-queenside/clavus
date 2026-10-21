//! Tests for the TUI key handler and application state.
//!
//! These exercise the pure logic of the TUI (navigation, screen transitions,
//! input handling) without needing a real terminal.

use clavus_tui::app::{App, Screen};
use clavus_tui::keys::{handle_key, key_char};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

// ─── Helpers ────────────────────────────────────────────────────────────────

/// A minimal but valid Ableton project.
const MINIMAL_ALS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton>
  <LiveSet>
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

struct Harness {
    app: App,
    _tmp: tempfile::TempDir,
}

impl Harness {
    /// Build an app rooted in a temp directory with one project.
    fn with_project() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let projects = tmp.path().join("projects");
        let data = tmp.path().join("data");
        std::fs::create_dir_all(&projects).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(projects.join("demo.als"), MINIMAL_ALS).unwrap();

        let mut app = App::new(projects);
        app.clavus_root = data;
        app.refresh_projects().unwrap();
        Self { app, _tmp: tmp }
    }

    fn empty() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let projects = tmp.path().join("projects");
        let data = tmp.path().join("data");
        std::fs::create_dir_all(&projects).unwrap();
        std::fs::create_dir_all(&data).unwrap();
        let mut app = App::new(projects);
        app.clavus_root = data;
        Self { app, _tmp: tmp }
    }
}

fn press(app: &mut App, code: KeyCode) -> bool {
    handle_key(&KeyEvent::new(code, KeyModifiers::NONE), app)
}

fn press_char(app: &mut App, c: char) -> bool {
    handle_key(&key_char(c), app)
}

// ─── Quit and navigation ────────────────────────────────────────────────────

#[test]
fn q_on_project_list_exits() {
    let mut h = Harness::empty();
    assert!(press_char(&mut h.app, 'q'), "q on the project list should exit");
}

#[test]
fn esc_on_project_list_exits() {
    let mut h = Harness::empty();
    assert!(press(&mut h.app, KeyCode::Esc), "Esc on the project list should exit");
}

#[test]
fn esc_on_sub_screen_returns_to_list() {
    let mut h = Harness::with_project();
    h.app.screen = Screen::CueList;
    assert!(!press(&mut h.app, KeyCode::Esc), "Esc should not exit from a sub-screen");
    assert_eq!(h.app.screen, Screen::ProjectList);
}

#[test]
fn esc_clears_messages() {
    let mut h = Harness::empty();
    h.app.set_info("hello".into());
    h.app.set_error("bad".into());
    h.app.screen = Screen::Sync;
    press(&mut h.app, KeyCode::Esc);
    assert!(h.app.info.is_none());
    assert!(h.app.error.is_none());
}

#[test]
fn down_selects_first_project_then_advances() {
    let mut h = Harness::with_project();
    assert!(h.app.selected_project.is_none());
    press(&mut h.app, KeyCode::Down);
    assert_eq!(h.app.selected_project, Some(0));
    // Only one project — selection must not run off the end.
    press(&mut h.app, KeyCode::Down);
    assert_eq!(h.app.selected_project, Some(0));
}

#[test]
fn up_at_top_stays_at_top() {
    let mut h = Harness::with_project();
    press(&mut h.app, KeyCode::Down);
    press(&mut h.app, KeyCode::Up);
    assert_eq!(h.app.selected_project, Some(0));
}

#[test]
fn down_on_empty_list_does_not_panic() {
    let mut h = Harness::empty();
    press(&mut h.app, KeyCode::Down);
    press(&mut h.app, KeyCode::Up);
    assert!(h.app.selected_project.is_none());
}

#[test]
fn enter_opens_project_detail() {
    let mut h = Harness::with_project();
    press(&mut h.app, KeyCode::Down);
    press(&mut h.app, KeyCode::Enter);
    assert_eq!(h.app.screen, Screen::ProjectDetail);
    assert!(h.app.project_detail.is_some());
    assert_eq!(h.app.project_detail.as_ref().unwrap().track_count, 1);
}

#[test]
fn enter_without_selection_stays_on_list() {
    let mut h = Harness::with_project();
    press(&mut h.app, KeyCode::Enter);
    assert_eq!(h.app.screen, Screen::ProjectList);
    assert!(h.app.project_detail.is_none());
}

#[test]
fn o_also_opens_project_detail() {
    let mut h = Harness::with_project();
    press(&mut h.app, KeyCode::Down);
    press_char(&mut h.app, 'o');
    assert_eq!(h.app.screen, Screen::ProjectDetail);
}

// ─── Screen transitions ─────────────────────────────────────────────────────

#[test]
fn s_opens_sync_screen_from_list() {
    let mut h = Harness::with_project();
    press_char(&mut h.app, 's');
    assert_eq!(h.app.screen, Screen::Sync);
}

#[test]
fn s_opens_sync_screen_from_detail() {
    let mut h = Harness::with_project();
    press(&mut h.app, KeyCode::Down);
    press(&mut h.app, KeyCode::Enter);
    press_char(&mut h.app, 's');
    assert_eq!(h.app.screen, Screen::Sync);
}

#[test]
fn l_opens_cue_list_from_detail() {
    let mut h = Harness::with_project();
    press(&mut h.app, KeyCode::Down);
    press(&mut h.app, KeyCode::Enter);
    press_char(&mut h.app, 'l');
    assert_eq!(h.app.screen, Screen::CueList);
}

#[test]
fn n_opens_snapshot_list_from_detail() {
    let mut h = Harness::with_project();
    press(&mut h.app, KeyCode::Down);
    press(&mut h.app, KeyCode::Enter);
    press_char(&mut h.app, 'n');
    assert_eq!(h.app.screen, Screen::SnapshotList);
}

#[test]
fn c_in_detail_shows_info_not_snapshot() {
    let mut h = Harness::with_project();
    press(&mut h.app, KeyCode::Down);
    press(&mut h.app, KeyCode::Enter);
    press_char(&mut h.app, 'c');
    assert_eq!(h.app.info.as_deref(), Some("snapshot: use CLI"));
}

// ─── Cue list ───────────────────────────────────────────────────────────────

#[test]
fn a_enters_cue_add_mode() {
    let mut h = Harness::with_project();
    h.app.screen = Screen::CueList;
    press_char(&mut h.app, 'a');
    assert_eq!(h.app.screen, Screen::CueAdd);
}

#[test]
fn cue_filter_cycles_through_statuses() {
    use clavus_core::types::CueStatus;
    let mut h = Harness::with_project();
    h.app.screen = Screen::CueList;

    press_char(&mut h.app, 'f');
    assert_eq!(h.app.cue_filter, Some(CueStatus::Pending));
    press_char(&mut h.app, 'f');
    assert_eq!(h.app.cue_filter, Some(CueStatus::Resolved));
    press_char(&mut h.app, 'f');
    assert_eq!(h.app.cue_filter, Some(CueStatus::Skipped));
    press_char(&mut h.app, 'f');
    assert!(h.app.cue_filter.is_none());
}

#[test]
fn r_in_cue_list_shows_info() {
    let mut h = Harness::with_project();
    h.app.screen = Screen::CueList;
    press_char(&mut h.app, 'r');
    assert_eq!(h.app.info.as_deref(), Some("cue resolve: use CLI"));
}

#[test]
fn cue_navigation_on_empty_list_does_not_panic() {
    let mut h = Harness::with_project();
    h.app.screen = Screen::CueList;
    press(&mut h.app, KeyCode::Down);
    press(&mut h.app, KeyCode::Up);
    assert!(h.app.selected_cue.is_none());
}

// ─── Cue add ────────────────────────────────────────────────────────────────

#[test]
fn typing_in_cue_add_appends_to_input_buffer() {
    let mut h = Harness::with_project();
    h.app.screen = Screen::CueAdd;
    for c in "1.1.1".chars() {
        press_char(&mut h.app, c);
    }
    assert_eq!(h.app.input_buf, "1.1.1");
}

#[test]
fn backspace_removes_last_character() {
    let mut h = Harness::with_project();
    h.app.screen = Screen::CueAdd;
    press_char(&mut h.app, 'a');
    press_char(&mut h.app, 'b');
    press(&mut h.app, KeyCode::Backspace);
    assert_eq!(h.app.input_buf, "a");
}

#[test]
fn backspace_on_empty_buffer_does_not_panic() {
    let mut h = Harness::with_project();
    h.app.screen = Screen::CueAdd;
    press(&mut h.app, KeyCode::Backspace);
    assert!(h.app.input_buf.is_empty());
}

#[test]
fn enter_in_cue_add_saves_cue_and_returns_to_list() {
    let mut h = Harness::with_project();
    press(&mut h.app, KeyCode::Down);
    press(&mut h.app, KeyCode::Enter);
    press_char(&mut h.app, 'l');
    press_char(&mut h.app, 'a');
    for c in "5.1.1".chars() {
        press_char(&mut h.app, c);
    }
    press(&mut h.app, KeyCode::Enter);

    assert_eq!(h.app.screen, Screen::CueList);
    assert!(h.app.input_buf.is_empty());
    assert_eq!(h.app.info.as_deref(), Some("cue saved"));
    assert_eq!(h.app.cues.len(), 1);
    assert_eq!(h.app.cues[0].position, "5.1.1");
}

#[test]
fn enter_with_empty_input_does_not_save() {
    let mut h = Harness::with_project();
    press(&mut h.app, KeyCode::Down);
    press(&mut h.app, KeyCode::Enter);
    press_char(&mut h.app, 'l');
    press_char(&mut h.app, 'a');
    press(&mut h.app, KeyCode::Enter);

    assert_eq!(h.app.screen, Screen::CueList);
    assert!(h.app.cues.is_empty());
    assert_ne!(h.app.info.as_deref(), Some("cue saved"));
}

#[test]
fn cue_save_uses_app_data_root_not_home() {
    // Regression: the TUI used to read cues from ~/.clavus while saving to
    // the CLI-provided root, so cues appeared to vanish.
    let mut h = Harness::with_project();
    press(&mut h.app, KeyCode::Down);
    press(&mut h.app, KeyCode::Enter);
    press_char(&mut h.app, 'l');
    press_char(&mut h.app, 'a');
    press_char(&mut h.app, '9');
    press(&mut h.app, KeyCode::Enter);

    // The cue must exist under the temp data root.
    let cue_dir = h.app.clavus_root.join("cues").join("demo");
    assert!(cue_dir.exists(), "cue dir should be under the app data root");
    let mut found = false;
    for entry in std::fs::read_dir(&cue_dir).unwrap().flatten() {
        if entry.path().extension().and_then(|s| s.to_str()) == Some("json") {
            let body = std::fs::read_to_string(entry.path()).unwrap();
            if body.contains("9") {
                found = true;
            }
        }
    }
    assert!(found, "saved cue should be persisted under the app data root");
}

// ─── Sync screen ────────────────────────────────────────────────────────────

#[test]
fn push_without_connection_shows_hint() {
    let mut h = Harness::with_project();
    h.app.screen = Screen::Sync;
    press_char(&mut h.app, 'P');
    assert_eq!(h.app.info.as_deref(), Some("press c to connect first"));
}

#[test]
fn pull_without_connection_shows_hint() {
    let mut h = Harness::with_project();
    h.app.screen = Screen::Sync;
    press_char(&mut h.app, 'G');
    assert_eq!(h.app.info.as_deref(), Some("press c to connect first"));
}

#[test]
fn connect_to_dead_relay_reports_failure() {
    // Regression: the TUI used to report "connected" optimistically and
    // dropped the result channel, so a failed connect was never surfaced.
    let mut h = Harness::with_project();
    h.app.screen = Screen::Sync;

    press_char(&mut h.app, 'c');
    assert!(!h.app.sync_state.is_connected(), "must not claim to be connected");
    assert_eq!(h.app.info.as_deref(), Some("connecting to 127.0.0.1:4477..."));

    // Give the background thread a moment to report the failure.
    let mut failed = false;
    for _ in 0..100 {
        h.app.poll_sync();
        if h.app.error.is_some() {
            failed = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(failed, "connect failure should be reported to the user");
    assert!(
        h.app.error.as_deref().unwrap().contains("connect failed"),
        "error should mention the failure, got: {:?}",
        h.app.error
    );
    assert!(!h.app.sync_state.is_connected());
}

#[test]
fn disconnect_when_not_connected_is_harmless() {
    let mut h = Harness::with_project();
    h.app.screen = Screen::Sync;
    press_char(&mut h.app, 'c');
    press_char(&mut h.app, 'c');
    assert!(!h.app.sync_state.is_connected());
}

#[test]
fn poll_sync_with_no_thread_is_a_noop() {
    let mut h = Harness::with_project();
    h.app.poll_sync();
    assert!(h.app.info.is_none());
    assert!(h.app.error.is_none());
}

// ─── Snapshot list ──────────────────────────────────────────────────────────

#[test]
fn snapshot_navigation_on_empty_list_does_not_panic() {
    let mut h = Harness::with_project();
    h.app.screen = Screen::SnapshotList;
    press(&mut h.app, KeyCode::Down);
    press(&mut h.app, KeyCode::Up);
    assert!(h.app.selected_snapshot.is_none());
}

// ─── Unknown keys ───────────────────────────────────────────────────────────

#[test]
fn unhandled_keys_are_ignored() {
    let mut h = Harness::empty();
    let before = h.app.screen;
    for c in "xyz!@#$%^&*()".chars() {
        assert!(!press_char(&mut h.app, c));
    }
    assert_eq!(h.app.screen, before);
}

#[test]
fn uppercase_q_does_not_exit() {
    let mut h = Harness::empty();
    assert!(!press_char(&mut h.app, 'Q'));
    assert_eq!(h.app.screen, Screen::ProjectList);
}

#[test]
fn modified_keys_are_ignored() {
    // Ctrl-C should not be treated as 'c' (no accidental snapshot).
    let mut h = Harness::with_project();
    press(&mut h.app, KeyCode::Down);
    press(&mut h.app, KeyCode::Enter);
    let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert!(!handle_key(&key, &mut h.app));
    assert!(h.app.info.is_none());
}

// ─── App state ──────────────────────────────────────────────────────────────

#[test]
fn refresh_projects_finds_both_project_types() {
    let tmp = tempfile::tempdir().unwrap();
    let projects = tmp.path().join("projects");
    std::fs::create_dir_all(&projects).unwrap();
    std::fs::write(projects.join("a.als"), MINIMAL_ALS).unwrap();
    std::fs::write(projects.join("b.bwproject"), b"not a real archive").unwrap();
    std::fs::write(projects.join("notes.txt"), b"ignore me").unwrap();

    let mut app = App::new(projects);
    app.clavus_root = tmp.path().join("data");
    app.refresh_projects().unwrap();

    assert_eq!(app.projects.len(), 2, "only .als and .bwproject should be listed");
    let names: Vec<&str> = app.projects.iter().map(|p| p.name.as_str()).collect();
    assert!(names.contains(&"a"));
    assert!(names.contains(&"b"));
    let types: Vec<&str> = app.projects.iter().map(|p| p.project_type.as_str()).collect();
    assert!(types.contains(&"Ableton"));
    assert!(types.contains(&"Bitwig"));
}

#[test]
fn refresh_projects_on_missing_dir_returns_error() {
    let mut app = App::new(std::path::PathBuf::from("/nonexistent/dir/xyz"));
    assert!(app.refresh_projects().is_err());
}

#[test]
fn load_project_with_bad_index_returns_error() {
    let mut h = Harness::with_project();
    assert!(h.app.load_project(99).is_err());
}

#[test]
fn load_project_with_broken_file_returns_error() {
    let tmp = tempfile::tempdir().unwrap();
    let projects = tmp.path().join("projects");
    std::fs::create_dir_all(&projects).unwrap();
    std::fs::write(projects.join("broken.als"), b"<not valid xml").unwrap();
    let mut app = App::new(projects);
    app.clavus_root = tmp.path().join("data");
    app.refresh_projects().unwrap();
    assert!(app.load_project(0).is_err());
    assert!(app.project_detail.is_none());
}

#[test]
fn selected_entry_reflects_selection() {
    let mut h = Harness::with_project();
    assert!(h.app.selected_entry().is_none());
    press(&mut h.app, KeyCode::Down);
    assert_eq!(h.app.selected_entry().unwrap().name, "demo");
}

// ─── Sync: storing what the relay sends ───────────────────────────────────

#[test]
fn store_pulled_saves_snapshots_and_imports_cues() {
    use clavus_tui::app::store_pulled;

    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&data).unwrap();

    // A snapshot with a real-shaped hash.
    let hash = "a".repeat(64);
    let snap = clavus_core::types::Snapshot {
        hash: hash.clone(),
        parent: None,
        timestamp: 1.0,
        message: "from a relay".to_string(),
        track_count: 2,
        bpm: 128.0,
        tags: vec![],
        notes: String::new(),
        sample_hashes: vec![],
        sample_paths: std::collections::HashMap::new(),
        project_path: "/tmp/demo.als".to_string(),
    };
    let cue = clavus_core::types::Cue {
        id: "abc123456789".to_string(),
        project: "demo".to_string(),
        position: "1.1.1".to_string(),
        text: "a note from a collaborator".to_string(),
        author: "slowhands".to_string(),
        timestamp: 1.0,
        status: clavus_core::types::CueStatus::Pending,
        snapshot_hash: String::new(),
        track_name: String::new(),
        assignee: String::new(),
        in_progress: false,
        replies: vec![],
    };

    let (stored, imported) = store_pulled(&[snap], &[cue], &data, "demo");
    assert_eq!(stored, 1, "the snapshot should be stored");
    assert_eq!(imported, 1, "the cue should be imported");

    // The snapshot should now be in the local chain.
    let blobs = clavus_core::blob::BlobStore::new(data.join("blobs")).unwrap();
    let store = clavus_core::snapshot::SnapshotStore::new(blobs, "demo");
    assert!(store.has(&hash), "the snapshot should be in the local store");
    assert_eq!(store.chain_length(None), 1, "HEAD should have advanced");

    // And the cue should be in the local cue store.
    let cues = clavus_core::CueStore::new(&data, "demo").unwrap();
    let listed = cues.list(None).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].text, "a note from a collaborator");
}

#[test]
fn store_pulled_rejects_a_snapshot_with_a_crafted_hash() {
    use clavus_tui::app::store_pulled;

    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&data).unwrap();

    let snap = clavus_core::types::Snapshot {
        hash: "../../escaped".to_string(),
        parent: None,
        timestamp: 1.0,
        message: "hostile".to_string(),
        track_count: 0,
        bpm: 120.0,
        tags: vec![],
        notes: String::new(),
        sample_hashes: vec![],
        sample_paths: std::collections::HashMap::new(),
        project_path: "/tmp/evil.als".to_string(),
    };

    let (stored, _) = store_pulled(&[snap], &[], &data, "demo");
    assert_eq!(stored, 0, "a crafted hash must not be stored");
    assert!(
        !tmp.path().join("escaped.meta").exists(),
        "nothing may be written outside the store"
    );
}

#[test]
fn store_pulled_rejects_a_cue_with_a_crafted_id() {
    use clavus_tui::app::store_pulled;

    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&data).unwrap();

    let cue = clavus_core::types::Cue {
        id: "../../escaped".to_string(),
        project: "demo".to_string(),
        position: "1.1.1".to_string(),
        text: "hostile".to_string(),
        author: "attacker".to_string(),
        timestamp: 1.0,
        status: clavus_core::types::CueStatus::Pending,
        snapshot_hash: String::new(),
        track_name: String::new(),
        assignee: String::new(),
        in_progress: false,
        replies: vec![],
    };

    let (_, imported) = store_pulled(&[], &[cue], &data, "demo");
    assert_eq!(imported, 0, "a crafted cue id must not be imported");
    assert!(
        !tmp.path().join("escaped.json").exists(),
        "nothing may be written outside the cue directory"
    );
}

#[test]
fn pushing_includes_the_projects_cues() {
    let mut h = Harness::with_project();
    // Select the project.
    press(&mut h.app, KeyCode::Down);
    // Add a cue for the selected project.
    let store = clavus_core::CueStore::new(&h.app.clavus_root, "demo").unwrap();
    store
        .add("a note worth sharing", "1.1.1", "chris", "", "Drums")
        .unwrap();

    let cues = h.app.project_cues();
    assert_eq!(cues.len(), 1, "the project's cues should be picked up");
    assert_eq!(cues[0].text, "a note worth sharing");
}

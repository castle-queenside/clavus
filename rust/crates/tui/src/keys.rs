//! Key handling for the Clavus TUI.
//!
//! Kept in the library (rather than the binary) so it can be unit tested
//! without a terminal.

use crate::app::{App, Screen, SyncCmd};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Handle a key press.
///
/// Returns `true` when the application should exit.
pub fn handle_key(key: &KeyEvent, app: &mut App) -> bool {
    match (key.code, key.modifiers) {
        // ── Quit / back ──────────────────────────────────
        (KeyCode::Esc, _) | (KeyCode::Char('q'), KeyModifiers::NONE) => {
            if app.screen == Screen::ProjectList {
                return true;
            }
            app.screen = Screen::ProjectList;
            app.clear_messages();
        }

        // ── Navigation ────────────────────────────────
        (KeyCode::Enter, _) | (KeyCode::Char('o'), KeyModifiers::NONE)
            if app.screen == Screen::ProjectList =>
        {
            if let Some(idx) = app.selected_project {
                if app.load_project(idx).is_ok() {
                    app.screen = Screen::ProjectDetail;
                }
            }
        }
        (KeyCode::Down, _) if app.screen == Screen::ProjectList => {
            if let Some(i) = app.selected_project {
                if i + 1 < app.projects.len() {
                    app.selected_project = Some(i + 1);
                }
            } else if !app.projects.is_empty() {
                app.selected_project = Some(0);
            }
        }
        (KeyCode::Up, _) if app.screen == Screen::ProjectList => {
            if let Some(i) = app.selected_project {
                if i > 0 {
                    app.selected_project = Some(i - 1);
                }
            }
        }

        // ── Project Detail ───────────────────────────
        (KeyCode::Char('c'), KeyModifiers::NONE) if app.screen == Screen::ProjectDetail => {
            app.set_info("snapshot: use CLI".into());
        }
        (KeyCode::Char('l'), KeyModifiers::NONE) if app.screen == Screen::ProjectDetail => {
            if app.load_cues().is_ok() {
                app.screen = Screen::CueList;
            }
        }
        (KeyCode::Char('n'), KeyModifiers::NONE) if app.screen == Screen::ProjectDetail => {
            if app.load_snapshots().is_ok() {
                app.screen = Screen::SnapshotList;
            }
        }
        (KeyCode::Char('s'), KeyModifiers::NONE)
            if app.screen == Screen::ProjectList || app.screen == Screen::ProjectDetail =>
        {
            app.screen = Screen::Sync;
        }

        // ── Cue List ───────────────────────────────
        (KeyCode::Down, _) if app.screen == Screen::CueList => {
            if let Some(i) = app.selected_cue {
                if i + 1 < app.cues.len() {
                    app.selected_cue = Some(i + 1);
                }
            } else if !app.cues.is_empty() {
                app.selected_cue = Some(0);
            }
        }
        (KeyCode::Up, _) if app.screen == Screen::CueList => {
            if let Some(i) = app.selected_cue {
                if i > 0 {
                    app.selected_cue = Some(i - 1);
                }
            }
        }
        (KeyCode::Char('a'), KeyModifiers::NONE) if app.screen == Screen::CueList => {
            app.screen = Screen::CueAdd;
        }
        (KeyCode::Char('r'), KeyModifiers::NONE) if app.screen == Screen::CueList => {
            app.set_info("cue resolve: use CLI".into());
        }
        (KeyCode::Char('f'), KeyModifiers::NONE) if app.screen == Screen::CueList => {
            use clavus_core::types::CueStatus;
            app.cue_filter = match app.cue_filter {
                None => Some(CueStatus::Pending),
                Some(CueStatus::Pending) => Some(CueStatus::Resolved),
                Some(CueStatus::Resolved) => Some(CueStatus::Skipped),
                _ => None,
            };
            let _ = app.load_cues();
        }

        // ── Cue Add ──────────────────────────────
        (KeyCode::Enter, _) if app.screen == Screen::CueAdd => {
            save_cue(app);
            app.input_buf.clear();
            app.screen = Screen::CueList;
        }
        (KeyCode::Char(c), KeyModifiers::NONE) if app.screen == Screen::CueAdd => {
            app.input_buf.push(c);
        }
        (KeyCode::Backspace, _) if app.screen == Screen::CueAdd => {
            app.input_buf.pop();
        }

        // ── Sync ──────────────────────────────────
        (KeyCode::Char('c'), KeyModifiers::NONE) if app.screen == Screen::Sync => {
            if app.sync_state.is_connected() {
                app.sync_disconnect();
            } else {
                app.sync_connect("127.0.0.1:4477");
            }
        }
        (KeyCode::Char('P'), KeyModifiers::NONE) if app.screen == Screen::Sync => {
            if app.sync_state.is_connected() {
                // Push the selected project's local snapshot chain.
                let snapshots = app.selected_snapshots();
                let n = snapshots.len();
                if snapshots.is_empty() {
                    app.set_info("no local snapshots to push".into());
                } else {
                    app.sync_state.send_cmd(SyncCmd::Push(snapshots));
                    app.set_info(format!("pushing {} snapshot(s)...", n));
                }
            } else {
                app.set_info("press c to connect first".into());
            }
        }
        (KeyCode::Char('G'), KeyModifiers::NONE) if app.screen == Screen::Sync => {
            if app.sync_state.is_connected() {
                let heads = app.known_heads();
                app.sync_state.send_cmd(SyncCmd::Pull(heads));
                app.set_info("pulling...".into());
            } else {
                app.set_info("press c to connect first".into());
            }
        }

        // ── Snapshot List ────────────────────────
        (KeyCode::Down, _) if app.screen == Screen::SnapshotList => {
            if let Some(i) = app.selected_snapshot {
                if i + 1 < app.snapshots.len() {
                    app.selected_snapshot = Some(i + 1);
                }
            } else if !app.snapshots.is_empty() {
                app.selected_snapshot = Some(0);
            }
        }
        (KeyCode::Up, _) if app.screen == Screen::SnapshotList => {
            if let Some(i) = app.selected_snapshot {
                if i > 0 {
                    app.selected_snapshot = Some(i - 1);
                }
            }
        }

        _ => {}
    }
    false
}

/// Persist the cue currently being typed into the cue store.
fn save_cue(app: &mut App) {
    if app.input_buf.is_empty() || app.project_detail.is_none() {
        return;
    }
    // Use the app's data root so `--data` / `CLAVUS_DATA` is honoured.
    let root = app.clavus_root.clone();
    let name = app
        .project_detail
        .as_ref()
        .and_then(|p| p.path.file_stem())
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");
    if let Ok(store) = clavus_core::CueStore::new(&root, name) {
        if store
            .add("", &app.input_buf, "tui", "", "Arrangement")
            .is_ok()
        {
            app.set_info("cue saved".into());
            let _ = app.load_cues();
        }
    }
}

/// Build a key event for a plain character press.
pub fn key_char(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

/// Build a key event for a named key.
pub fn key_code(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

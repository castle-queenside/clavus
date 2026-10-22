//! Key handling for the Clavus TUI.
//!
//! Kept in the library (rather than the binary) so it can be unit tested
//! without a terminal.

use crate::app::{App, CueStep, Screen, SyncCmd};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Handle a key press.
///
/// Returns `true` when the application should exit.
pub fn handle_key(key: &KeyEvent, app: &mut App) -> bool {
    // While a text field has the focus, the field owns the keyboard.
    //
    // The quit/back key used to be handled first, so typing a "q" in the
    // cue form jumped back to the project list and threw the text away.
    // No word containing a lowercase "q" could be typed.
    if app.screen == Screen::CueAdd || app.screen == Screen::RelayInput {
        return handle_text_input(key, app);
    }

    match (key.code, key.modifiers) {
        // ── Quit / back ──────────────────────────────────
        //
        // Ctrl-C quits. The terminal is in raw mode, so the interrupt
        // arrives as an ordinary key and never reaches the shell: a
        // user who pressed Ctrl-C and saw the app carry on would think
        // the program had hung.
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            return true;
        }
        (KeyCode::Esc, _) | (KeyCode::Char('q'), KeyModifiers::NONE) => {
            if app.screen == Screen::ProjectList {
                return true;
            }
            // Esc goes back one level, so the user can walk back through
            // the screens instead of always landing on the project list.
            app.screen = app.previous_screen();
            app.clear_messages();
        }

        // ── Navigation ────────────────────────────────
        (KeyCode::Enter, _) | (KeyCode::Char('o'), KeyModifiers::NONE)
            if app.screen == Screen::ProjectList =>
        {
            // Enter with nothing selected used to do nothing at all:
            // no redraw, no message. The footer says "Enter open", so
            // the first project is selected and opened.
            let idx = match app.selected_project {
                Some(idx) => idx,
                None if !app.projects.is_empty() => {
                    app.selected_project = Some(0);
                    0
                }
                None => {
                    app.set_info("no projects to open".into());
                    return false;
                }
            };
            match app.load_project(idx) {
                Ok(()) => app.screen = Screen::ProjectDetail,
                // A project that cannot be parsed used to do nothing
                // at all — no error, no message. The user could not
                // tell whether the key had been pressed.
                Err(e) => app.set_error(e),
            }
        }
        (KeyCode::Down, _) if app.screen == Screen::ProjectList => {
            // The counts in the list are read from the store, and the
            // store changes when the user works in the CLI. The list
            // was only read once at start-up, so the TUI and the CLI
            // disagreed about the same project until the TUI was
            // restarted. It is refreshed as the user moves through it.
            app.refresh_project_counts();
            if let Some(i) = app.selected_project {
                if i + 1 < self_len(app) {
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
            match app.load_cues() {
                Ok(()) => app.screen = Screen::CueList,
                Err(e) => app.set_error(e),
            }
        }
        (KeyCode::Char('n'), KeyModifiers::NONE) if app.screen == Screen::ProjectDetail => {
            match app.load_snapshots() {
                Ok(()) => app.screen = Screen::SnapshotList,
                Err(e) => app.set_error(e),
            }
        }
        (KeyCode::Char('s'), KeyModifiers::NONE)
            if app.screen == Screen::ProjectList || app.screen == Screen::ProjectDetail =>
        {
            app.open_sync();
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
            app.cue_step = CueStep::Message;
            app.cue_message.clear();
            app.input_buf.clear();
        }
        (KeyCode::Char('r'), KeyModifiers::NONE) if app.screen == Screen::CueList => {
            app.set_info("cue resolve: use CLI".into());
        }
        (KeyCode::Char('f'), KeyModifiers::NONE) if app.screen == Screen::CueList => {
            use clavus_core::types::CueStatus;
            // Every status is reachable. The filter used to stop after
            // "skipped", so deferred and archived cues were invisible.
            app.cue_filter = match app.cue_filter {
                None => Some(CueStatus::Pending),
                Some(CueStatus::Pending) => Some(CueStatus::Resolved),
                Some(CueStatus::Resolved) => Some(CueStatus::Skipped),
                Some(CueStatus::Skipped) => Some(CueStatus::Deferred),
                Some(CueStatus::Deferred) => Some(CueStatus::Archived),
                _ => None,
            };
            if let Err(e) = app.load_cues() {
                app.set_error(e);
            }
        }

        // ── Sync ──────────────────────────────────
        (KeyCode::Char('c'), KeyModifiers::NONE) if app.screen == Screen::Sync => {
            if app.sync_state.is_connected() {
                app.sync_disconnect();
            } else {
                let addr = app.relay_address().to_string();
                app.sync_connect(&addr);
            }
        }
        (KeyCode::Char('r'), KeyModifiers::NONE) if app.screen == Screen::Sync => {
            // Type a relay address. The TUI used to be able to talk to
            // 127.0.0.1:4477 and nothing else, so a collaborator on
            // another machine could not use the TUI at all.
            app.screen = Screen::RelayInput;
            // The field starts empty. It used to be prefilled with the
            // old address, so typing a new one appended to it and the
            // result was a nonsense address that could never connect.
            app.input_buf.clear();
        }
        (KeyCode::Char('P'), KeyModifiers::NONE) if app.screen == Screen::Sync => {
            if app.sync_state.is_connected() {
                // Push the selected project's local snapshot chain and
                // its notes.
                let snapshots = app.selected_snapshots();
                let n = snapshots.len();
                // The cues travel with the snapshots — a collaborator's
                // notes are the point of the cue system. A project with
                // notes but no snapshot is still worth sharing: the CLI
                // sends the notes on their own, and the TUI used to
                // refuse, so a cue-only user could share nothing.
                let cues = app.project_cues();
                let c = cues.len();
                if snapshots.is_empty() && cues.is_empty() {
                    app.set_info("nothing to push — no local snapshots and no notes".into());
                } else {
                    // The relay keys its storage by project, so the push
                    // has to say which project these belong to.
                    let project = app.selected_project_sync_name();
                    // The project bytes travel with the snapshots, so
                    // the collaborator can restore them on their own
                    // machine and not only read the history.
                    let blobs = app.selected_snapshot_bytes();
                    app.sync_state.send_cmd(SyncCmd::Push {
                        snapshots,
                        cues,
                        blobs,
                        project,
                    });
                    if n == 0 {
                        app.set_info(format!("pushing {} note(s) only...", c));
                    } else {
                        app.set_info(format!("pushing {} snapshot(s), {} cue(s)...", n, c));
                    }
                }
            } else {
                app.set_info("press c to connect first".into());
            }
        }
        (KeyCode::Char('G'), KeyModifiers::NONE) if app.screen == Screen::Sync => {
            if app.sync_state.is_connected() {
                let heads = app.known_heads();
                let project_path = app
                    .selected_entry()
                    .map(|e| e.path.clone())
                    .unwrap_or_default();
                let clavus_root = app.clavus_root.clone();
                app.sync_state.send_cmd(SyncCmd::Pull {
                    known_heads: heads,
                    project_path,
                    clavus_root,
                });
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

/// The number of projects in the list.
fn self_len(app: &App) -> usize {
    app.projects.len()
}

/// Handle a key press while a text field has the focus.
fn handle_text_input(key: &KeyEvent, app: &mut App) -> bool {
    match (key.code, key.modifiers) {
        // Ctrl-C quits from anywhere, even from a text field.
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
            return true;
        }
        (KeyCode::Esc, _) => {
            // Cancelling throws the typed text away — it used to stay in
            // the buffer, so the next cue started with the text of the
            // cue that was cancelled.
            app.input_buf.clear();
            app.cue_message.clear();
            app.screen = app.previous_screen();
            app.clear_messages();
        }
        (KeyCode::Enter, _) => {
            if app.screen == Screen::RelayInput {
                let addr = app.input_buf.trim().to_string();
                if !addr.is_empty() {
                    app.set_relay_address(addr.clone());
                    app.screen = Screen::Sync;
                    app.sync_connect(&addr);
                } else {
                    app.screen = Screen::Sync;
                }
            } else {
                // The cue form asks for the message first and the
                // position second, so the cue ends up in the same shape
                // the CLI writes it.
                match app.cue_step {
                    CueStep::Message => {
                        app.cue_message = app.input_buf.trim().to_string();
                        app.cue_step = CueStep::Position;
                        // The position field starts empty; an empty
                        // position means the default. Pre-filling it
                        // meant the user's own typing was appended to
                        // the default and the position was nonsense.
                        app.input_buf.clear();
                    }
                    CueStep::Position => {
                        save_cue(app);
                        app.screen = Screen::CueList;
                    }
                }
            }
        }
        (KeyCode::Char(c), KeyModifiers::NONE) => {
            app.input_buf.push(c);
        }
        (KeyCode::Backspace, _) => {
            app.input_buf.pop();
        }
        _ => {}
    }
    false
}

/// Persist the cue currently being typed into the cue store.
///
/// The message and the position used to be passed the wrong way round,
/// so every cue made in the TUI was stored with an empty message and
/// the message in the position field. The CLI listed them as blank
/// rows.
fn save_cue(app: &mut App) {
    let text = app.cue_message.trim().to_string();
    let position = {
        let p = app.input_buf.trim();
        if p.is_empty() {
            crate::app::DEFAULT_CUE_POSITION.to_string()
        } else {
            p.to_string()
        }
    };
    if text.is_empty() || app.project_detail.is_none() {
        app.set_error("a cue needs a message".into());
        return;
    }
    if !is_valid_position(&position) {
        app.set_error(format!(
            "'{}' is not a valid position — expected bars.beats.sixteenths",
            position
        ));
        return;
    }
    // Use the app's data root so `--data` / `CLAVUS_DATA` is honoured.
    let root = app.clavus_root.clone();
    let project = app
        .project_detail
        .as_ref()
        .map(|p| clavus_core::project::ProjectId::new(&p.path));
    if let Some(project) = project {
        match clavus_core::CueStore::new(&root, &project) {
            Ok(store) => match store.add(
                &text,
                &position,
                &clavus_core::default_author(),
                "",
                "Arrangement",
            ) {
                Ok(_) => {
                    app.set_info("cue saved".into());
                    app.input_buf.clear();
                    let _ = app.load_cues();
                }
                Err(e) => app.set_error(format!("could not save the cue: {}", e)),
            },
            Err(e) => app.set_error(format!("cue store: {}", e)),
        }
    }
}

/// Whether a position is a `bars.beats.sixteenths` position.
fn is_valid_position(position: &str) -> bool {
    let parts: Vec<&str> = position.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

/// Build a key event for a plain character press.
pub fn key_char(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

/// Build a key event for a named key.
pub fn key_code(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

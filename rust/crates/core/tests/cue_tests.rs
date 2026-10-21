//! Tests for the cue store.
//!
//! Cues are collaborative review notes attached to a DAW timeline, so the
//! store has to survive unusual text, unusual identifiers, and repeated
//! writes without corrupting its index.

use clavus_core::cue::{CueStore, ImportOutcome};
use clavus_core::types::CueStatus;
use std::path::Path;
use tempfile::TempDir;

fn store(dir: &TempDir, project: &str) -> CueStore {
    CueStore::new(dir.path(), project).expect("cue store")
}

#[test]
fn add_and_list_a_cue() {
    let dir = TempDir::new().unwrap();
    let s = store(&dir, "demo");

    let cue = s
        .add("kick is too loud", "4.1.1", "chris", "abc123", "Drums")
        .expect("add");
    assert!(!cue.id.is_empty(), "cue should get an id");
    let id = cue.id;

    let cues = s.list(None).expect("list");
    assert_eq!(cues.len(), 1);
    assert_eq!(cues[0].position, "4.1.1");
    assert_eq!(cues[0].text, "kick is too loud");
    assert_eq!(cues[0].track_name, "Drums");
    assert_eq!(cues[0].status, CueStatus::Pending);
}

#[test]
fn filter_by_status() {
    let dir = TempDir::new().unwrap();
    let s = store(&dir, "demo");

    let a = s.add("one", "1.1.1", "chris", "", "Drums").unwrap().id;
    let b = s.add("two", "2.1.1", "chris", "", "Drums").unwrap().id;
    s.add("three", "3.1.1", "chris", "", "Drums").unwrap();

    s.resolve(&a, "fixed").unwrap();
    s.skip(&b, "not relevant").unwrap();

    let pending = s.list(Some("pending")).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].text, "three");

    let resolved = s.list(Some("resolved")).unwrap();
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].text, "one");

    let skipped = s.list(Some("skipped")).unwrap();
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].text, "two");

    assert_eq!(s.list(None).unwrap().len(), 3);
}

#[test]
fn count_pending_reflects_status() {
    let dir = TempDir::new().unwrap();
    let s = store(&dir, "demo");

    s.add("one", "1.1.1", "chris", "", "T").unwrap();
    s.add("two", "2.1.1", "chris", "", "T").unwrap();
    let c = s.add("three", "3.1.1", "chris", "", "T").unwrap().id;
    assert_eq!(s.count_pending().unwrap(), 3);

    s.resolve(&c, "done").unwrap();
    assert_eq!(s.count_pending().unwrap(), 2);
}

#[test]
fn resolve_marks_a_cue_resolved() {
    let dir = TempDir::new().unwrap();
    let s = store(&dir, "demo");
    let id = s.add("fix me", "1.1.1", "chris", "", "T").unwrap().id;

    let updated = s.resolve(&id, "done it").expect("resolve");
    assert!(updated.is_some());
    assert_eq!(updated.unwrap().status, CueStatus::Resolved);
}

#[test]
fn resolve_unknown_cue_is_an_error() {
    let dir = TempDir::new().unwrap();
    let s = store(&dir, "demo");
    // resolve, skip and assign all report a missing cue as an error.
    assert!(s.resolve("nonexistent", "x").is_err());
    assert!(s.skip("nonexistent", "x").is_err());
    assert!(s.assign("nonexistent", "x").is_err());
}

#[test]
fn delete_removes_a_cue() {
    let dir = TempDir::new().unwrap();
    let s = store(&dir, "demo");
    let id = s.add("temporary", "1.1.1", "chris", "", "T").unwrap().id;

    assert!(s.delete(&id), "delete should report success");
    assert!(s.list(None).unwrap().is_empty());
    assert!(!s.delete(&id), "deleting twice should report failure");
}

#[test]
fn reply_appends_to_a_cue() {
    let dir = TempDir::new().unwrap();
    let s = store(&dir, "demo");
    let id = s.add("question", "1.1.1", "chris", "", "T").unwrap().id;

    s.reply(&id, "answer", "slowhands", "").expect("reply");
    s.reply(&id, "more", "chris", "").expect("reply");

    let cues = s.list(None).unwrap();
    assert_eq!(cues.len(), 1);
    assert_eq!(cues[0].replies.len(), 2);
    assert_eq!(cues[0].replies[0].text, "answer");
    assert_eq!(cues[0].replies[1].author, "chris");
}

#[test]
fn assign_sets_the_assignee() {
    let dir = TempDir::new().unwrap();
    let s = store(&dir, "demo");
    let id = s.add("needs owner", "1.1.1", "chris", "", "T").unwrap().id;

    let updated = s.assign(&id, "slowhands").expect("assign");
    assert_eq!(updated.unwrap().assignee, "slowhands");
}

#[test]
fn cues_survive_a_store_reopen() {
    let dir = TempDir::new().unwrap();
    let id = {
        let s = store(&dir, "demo");
        s.add("persist me", "1.1.1", "chris", "", "T").unwrap().id
    };

    // A fresh store over the same root must see the cue.
    let s2 = store(&dir, "demo");
    let cues = s2.list(None).unwrap();
    assert_eq!(cues.len(), 1);
    assert_eq!(cues[0].id, id);
    assert_eq!(cues[0].text, "persist me");
}

#[test]
fn unicode_text_is_preserved() {
    let dir = TempDir::new().unwrap();
    let s = store(&dir, "demo");

    let text = "多重奏 — kick is too loud 🔊 «quotes» \"and\" 'apostrophes'";
    let id = s.add(text, "1.1.1", "chris 🎵", "", " drums & bass ").unwrap().id;

    let loaded = s.load(&id).unwrap().expect("cue should load");
    assert_eq!(loaded.text, text);
    assert_eq!(loaded.author, "chris 🎵");
    assert_eq!(loaded.track_name, " drums & bass ");
}

#[test]
fn newlines_in_cue_text_are_preserved() {
    let dir = TempDir::new().unwrap();
    let s = store(&dir, "demo");
    let text = "line one\nline two\ttabbed\r\nwindows";
    let id = s.add(text, "1.1.1", "chris", "", "T").unwrap().id;

    let loaded = s.load(&id).unwrap().unwrap();
    assert_eq!(loaded.text, text);
}

#[test]
fn very_long_cue_text_is_preserved() {
    let dir = TempDir::new().unwrap();
    let s = store(&dir, "demo");
    let text = "x".repeat(50_000);
    let id = s.add(&text, "1.1.1", "chris", "", "T").unwrap().id;

    let loaded = s.load(&id).unwrap().unwrap();
    assert_eq!(loaded.text.len(), 50_000);
}

#[test]
fn empty_cue_text_is_allowed() {
    let dir = TempDir::new().unwrap();
    let s = store(&dir, "demo");
    let id = s.add("", "1.1.1", "chris", "", "T").unwrap().id;
    let loaded = s.load(&id).unwrap().unwrap();
    assert_eq!(loaded.text, "");
}

#[test]
fn many_cues_are_all_stored() {
    let dir = TempDir::new().unwrap();
    let s = store(&dir, "demo");
    for i in 0..200 {
        s.add(&format!("cue {}", i), &format!("{}.1.1", i + 1), "chris", "", "T")
            .unwrap();
    }
    let cues = s.list(None).unwrap();
    assert_eq!(cues.len(), 200);
    assert_eq!(s.count_pending().unwrap(), 200);
}

#[test]
fn projects_are_stored_separately() {
    let dir = TempDir::new().unwrap();
    let a = store(&dir, "project-a");
    let b = store(&dir, "project-b");

    a.add("a cue", "1.1.1", "chris", "", "T").unwrap();
    b.add("b cue", "1.1.1", "chris", "", "T").unwrap();

    assert_eq!(a.list(None).unwrap().len(), 1);
    assert_eq!(a.list(None).unwrap()[0].text, "a cue");
    assert_eq!(b.list(None).unwrap()[0].text, "b cue");
}

#[test]
fn project_names_with_odd_characters_work() {
    let dir = TempDir::new().unwrap();
    // Names that would be awkward in a file path.
    for name in [
        "project with spaces",
        "project-with-dashes",
        "project.with.dots",
        "про_ject_123",
        "про-ject-🎵",
    ] {
        let s = CueStore::new(dir.path(), name).expect("store should be creatable");
        let id = s.add("cue", "1.1.1", "chris", "", "T").unwrap().id;
        let loaded = s.load(&id).unwrap().expect("cue should load");
        assert_eq!(loaded.text, "cue");
    }
}

#[test]
fn import_adds_a_remote_cue() {
    let dir = TempDir::new().unwrap();
    let s = store(&dir, "demo");

    let remote = clavus_core::types::Cue {
        id: "remote-1".to_string(),
        position: "5.1.1".to_string(),
        text: "from a teammate".to_string(),
        author: "slowhands".to_string(),
        timestamp: 1234.5,
        status: CueStatus::Pending,
        snapshot_hash: String::new(),
        track_name: "Bass".to_string(),
        assignee: String::new(),
        in_progress: false,
        replies: vec![],
    };

    let outcome = s.import(remote.clone()).expect("import");
    assert_eq!(outcome, ImportOutcome::Imported);
    let loaded = s.load(&remote.id).unwrap().expect("imported cue should load");
    assert_eq!(loaded.text, "from a teammate");
    assert_eq!(loaded.author, "slowhands");

    // Re-importing the identical cue reports "unchanged".
    assert_eq!(
        s.import(remote).expect("re-import"),
        ImportOutcome::Unchanged
    );
}

#[test]
fn listing_a_project_with_no_cues_returns_empty() {
    let dir = TempDir::new().unwrap();
    let s = store(&dir, "empty-project");
    assert!(s.list(None).unwrap().is_empty());
    assert_eq!(s.count_pending().unwrap(), 0);
}

#[test]
fn cue_store_creates_its_own_directory() {
    let dir = TempDir::new().unwrap();
    let nested = dir.path().join("a").join("b").join("c");
    let s = CueStore::new(&nested, "demo").expect("store should create dirs");
    s.add("cue", "1.1.1", "chris", "", "T").unwrap();
    assert!(nested.exists(), "cue root should have been created");
}

#[test]
fn corrupted_cue_file_does_not_panic() {
    let dir = TempDir::new().unwrap();
    let s = store(&dir, "demo");
    let id = s.add("cue", "1.1.1", "chris", "", "T").unwrap().id;

    // Corrupt the cue file on disk.
    let cue_dir = dir.path().join("cues").join("demo");
    let path = cue_dir.join(format!("{}.json", id));
    std::fs::write(&path, b"{ not valid json").unwrap();

    // Listing must not panic; the bad cue is skipped or reported.
    let result = s.list(None);
    assert!(result.is_ok() || result.is_err());
    if let Ok(cues) = result {
        assert!(cues.is_empty(), "corrupt cue should not be returned");
    }
}

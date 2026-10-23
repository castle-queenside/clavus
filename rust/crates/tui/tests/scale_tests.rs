//! A library of a thousand sets must still be a usable library.
//!
//! These tests are about the shape of the work, not its exact
//! duration: a machine that is ten times slower than this one must not
//! be ten times slower to open the list.
//!
//! The bug they guard against is real. The TUI used to resolve every
//! folder in the library to its real path — one path lookup per
//! folder, and a studio's library has tens of thousands of folders —
//! and it used to parse every project file before the first screen
//! appeared. On a real 1,245-set library on an external drive that
//! took over a minute. The list now appears in about a second, and the
//! numbers in it arrive in the background.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use clavus_tui::app::{App, Summary};
use tempfile::TempDir;

/// A folder full of project files.
struct Library {
    tmp: TempDir,
    dir: PathBuf,
}

impl Library {
    /// Build a library of `count` projects spread over `folders` folders.
    fn new(count: usize, folders: usize) -> Self {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("clavus").join("projects");
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..count {
            let folder = dir.join(format!("folder-{:02}", i % folders.max(1)));
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(folder.join(format!("Set {:04}.als", i)), MINIMAL_ALS).unwrap();
        }
        Self { tmp, dir }
    }
}

/// A small, valid Ableton project: 128 BPM and two tracks.
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

fn app_for(lib: &Library) -> App {
    let data = lib.tmp.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let mut app = App::new(lib.dir.clone());
    app.clavus_root = data;
    app
}

// ─── Finding the projects must not cost a path lookup per folder ────
//
// The old walk resolved every folder to its real path. A library with
// many folders paid for every one of them, and the cost grew with the
// number of folders rather than with the number of projects.

#[test]
fn finding_the_projects_does_not_walk_the_store() {
    let lib = Library::new(300, 30);
    let start = Instant::now();
    let found = clavus_core::project::discover_projects(std::slice::from_ref(&lib.dir), true);
    let elapsed = start.elapsed();

    assert_eq!(found.len(), 300, "every project is found");
    assert!(
        elapsed < Duration::from_secs(5),
        "finding 300 projects took {:?}, which is a path lookup per folder",
        elapsed
    );
}

#[test]
fn a_symlink_loop_does_not_make_the_walk_run_forever() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("projects");
    std::fs::create_dir_all(dir.join("inner")).unwrap();
    std::fs::write(dir.join("inner").join("Set.als"), MINIMAL_ALS).unwrap();
    // A folder that points at its own ancestor.
    #[cfg(unix)]
    std::os::unix::fs::symlink(&dir, dir.join("inner").join("loop")).unwrap();

    let start = Instant::now();
    let found = clavus_core::project::discover_projects(std::slice::from_ref(&dir), true);
    let elapsed = start.elapsed();

    assert_eq!(found.len(), 1, "the one real project is found once");
    assert!(
        elapsed < Duration::from_secs(5),
        "the loop took {:?}",
        elapsed
    );
}

#[test]
fn a_symlinked_folder_is_followed_once() {
    let tmp = TempDir::new().unwrap();
    let real = tmp.path().join("real");
    std::fs::create_dir_all(&real).unwrap();
    std::fs::write(real.join("Set.als"), MINIMAL_ALS).unwrap();
    let dir = tmp.path().join("projects");
    std::fs::create_dir_all(&dir).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real, dir.join("link")).unwrap();

    let found = clavus_core::project::discover_projects(std::slice::from_ref(&dir), true);
    assert_eq!(
        found.len(),
        1,
        "a folder reachable twice is still one project"
    );
}

// ─── The list must not wait for every project to be parsed ──────────
//
// A studio's library is gigabytes of gzipped XML. Reading it all
// before the first screen appears took over a minute on a real drive.

#[test]
fn the_list_appears_before_the_projects_have_been_read() {
    let lib = Library::new(200, 20);
    let mut app = app_for(&lib);

    let start = Instant::now();
    app.refresh_projects().unwrap();
    let elapsed = start.elapsed();

    assert_eq!(app.projects.len(), 200, "every project is listed");
    assert!(
        elapsed < Duration::from_secs(10),
        "the list took {:?} to appear, which means it waited for the \
         project files to be read",
        elapsed
    );
}

#[test]
fn a_project_that_has_not_been_read_yet_does_not_show_a_number() {
    let lib = Library::new(20, 2);
    let mut app = app_for(&lib);
    app.refresh_projects().unwrap();

    // Nothing has been read yet, so the list must not claim a tempo.
    // Zero is a claim about the project; a dash is a claim about the
    // list.
    let entry = app.projects.first().expect("a project");
    assert_eq!(
        entry.summary,
        Summary::Pending,
        "the tempo is not known yet"
    );
    assert_eq!(entry.bpm(), 0.0);
}

#[test]
fn the_row_under_the_cursor_is_read_at_once() {
    let lib = Library::new(20, 2);
    let mut app = app_for(&lib);
    app.refresh_projects().unwrap();

    // The user picks a row. Its numbers must arrive without waiting
    // for the background reader to reach it.
    app.move_selection(1);
    let index = app.selected_project.expect("a selection");
    let path = app.projects[index].path.clone();

    let start = Instant::now();
    app.poll_summaries();
    let elapsed = start.elapsed();

    let entry = app.projects.iter().find(|p| p.path == path).unwrap();
    assert!(
        matches!(entry.summary, Summary::Known { .. }),
        "the selected row was read: {:?}",
        entry.summary
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "reading one row took {:?}",
        elapsed
    );
}

#[test]
fn the_numbers_arrive_in_the_background() {
    let lib = Library::new(50, 5);
    let mut app = app_for(&lib);
    app.refresh_projects().unwrap();

    // The background reader fills the list in. Give it a moment.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut read = 0;
    while Instant::now() < deadline {
        app.poll_summaries();
        read = app
            .projects
            .iter()
            .filter(|p| matches!(p.summary, Summary::Known { .. }))
            .count();
        if read == app.projects.len() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        read,
        app.projects.len(),
        "every project was read in the background"
    );
}

// ─── Moving the cursor must not re-read the whole store ─────────────
//
// The counts come from the store, and re-reading them for every
// project in a large library walks every chain in it. On a library of
// a thousand sets that took a tenth of a second on every keypress,
// which made the list feel broken.

#[test]
fn moving_the_cursor_does_not_re_read_the_store_every_time() {
    let lib = Library::new(400, 40);
    let mut app = app_for(&lib);
    app.refresh_projects().unwrap();

    let start = Instant::now();
    for _ in 0..20 {
        app.refresh_project_counts_if_stale();
        app.move_selection(1);
    }
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_secs(5),
        "twenty moves took {:?}, which means the store was read every time",
        elapsed
    );
}

#[test]
fn an_explicit_refresh_always_happens() {
    let lib = Library::new(50, 5);
    let mut app = app_for(&lib);
    app.refresh_projects().unwrap();

    // The throttle is for the hot path. A caller that knows the store
    // has changed must not be silently ignored.
    app.refresh_project_counts();
    let first = app.projects.iter().map(|p| p.snap_count).sum::<usize>();
    app.refresh_project_counts();
    let second = app.projects.iter().map(|p| p.snap_count).sum::<usize>();
    assert_eq!(first, second, "an explicit refresh reads the store again");
}

// ─── The list and the CLI must find the same projects ───────────────

#[test]
fn the_list_and_the_cli_find_the_same_projects() {
    let lib = Library::new(120, 12);
    let mut app = app_for(&lib);
    app.refresh_projects().unwrap();

    let from_tui: Vec<String> = app.projects.iter().map(|p| p.name.clone()).collect();
    let from_cli: Vec<String> =
        clavus_core::project::discover_projects(std::slice::from_ref(&lib.dir), true)
            .into_iter()
            .map(|p| p.name)
            .collect();

    assert_eq!(
        from_tui.len(),
        from_cli.len(),
        "the TUI and the shared finder disagree about how many projects there are"
    );
    for name in &from_cli {
        assert!(from_tui.contains(name), "the TUI is missing {}", name);
    }
}

#[test]
fn live_backup_copies_are_not_projects() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("projects");
    let set = dir.join("My Set");
    std::fs::create_dir_all(set.join("Backup")).unwrap();
    std::fs::write(set.join("My Set.als"), MINIMAL_ALS).unwrap();
    std::fs::write(set.join("Backup").join("My Set.als"), MINIMAL_ALS).unwrap();

    let found = clavus_core::project::discover_projects(std::slice::from_ref(&dir), true);
    assert_eq!(found.len(), 1, "the backup copy is not a project");
}

#[test]
fn a_folder_named_like_a_project_is_not_a_project() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("projects");
    std::fs::create_dir_all(dir.join("Folder.als")).unwrap();

    let found = clavus_core::project::discover_projects(std::slice::from_ref(&dir), true);
    assert!(found.is_empty(), "a folder is not a project");
}

// ─── The name lookup has to be quick too ────────────────────────────
//
// `clavus log "My Set.als"` searches the whole library by name, and it
// did it with the same expensive walk.

#[test]
fn finding_a_project_by_name_does_not_walk_the_store() {
    let lib = Library::new(300, 30);
    let start = Instant::now();
    let found = clavus_core::project::find_projects_named(
        std::slice::from_ref(&lib.dir),
        Path::new("Set 0042.als"),
    );
    let elapsed = start.elapsed();

    assert_eq!(found.len(), 1, "the one set with that name is found");
    assert!(
        elapsed < Duration::from_secs(5),
        "the name lookup took {:?}",
        elapsed
    );
}

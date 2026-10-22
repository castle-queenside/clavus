//! The fifth beta wave: what a real library of sets exposed.
//!
//! Every test here came from running the compiled binary over a real
//! drive of Ableton sets — 2,690 `.als` files, made over several
//! machines and several years. Three things came out of it.

use clavus_core::als_refs;
use clavus_core::project::is_live_backup_copy;
use clavus_core::sample::{audit_project, is_factory_library_path};
use std::fs;
use std::path::{Path, PathBuf};

fn make_als(path: &Path, xml: &str) {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok();
    }
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    std::io::Write::write_all(&mut enc, xml.as_bytes()).ok();
    let compressed = enc.finish().unwrap_or_default();
    fs::write(path, &compressed).ok();
}

fn make_sample(path: &Path, content: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok();
    }
    fs::write(path, content).ok();
}

/// A Live 9 set whose sample is relative to the machine's *user
/// folder* rather than to the project. The leading empty
/// `RelativePathElement` stands for the user folder's own root.
fn live9_user_folder_als(sample_dir: &str, sample_name: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton>
  <LiveSet>
    <Tracks>
      <AudioTrack>
        <Name Value="Test" />
        <DeviceChain>
          <SampleRef><FileRef>
            <HasRelativePath Value="true" />
            <RelativePathType Value="1" />
            <RelativePath>
              <RelativePathElement Id="0" Dir="" />
              <RelativePathElement Id="1" Dir="{sample_dir}" />
            </RelativePath>
            <Name Value="{sample_name}" />
            <Type Value="1" />
          </FileRef></SampleRef>
        </DeviceChain>
      </AudioTrack>
    </Tracks>
  </LiveSet>
</Ableton>"#
    )
}

// ─── A user-folder path was looked for at the root of the filesystem ───────

#[test]
fn a_user_folder_path_is_not_looked_for_at_the_root_of_the_filesystem() {
    // A Live 9 set stores its sample path relative to the machine's
    // user folder, and the chain begins with an empty element standing
    // for that folder's root. Joined naively the path began with `//`,
    // and `//` is an absolute path on Unix — so the sample was looked
    // for at `/Samples/kick.wav`, at the very root of the filesystem,
    // and always came up missing.
    let xml = live9_user_folder_als("Samples", "kick.wav");
    let refs = als_refs::extract_sample_refs_from_xml(&xml);

    assert_eq!(refs.len(), 1, "one reference: {:?}", refs);
    let path = &refs[0];
    assert!(
        !path.is_absolute(),
        "a user-folder path must not be absolute: {:?}",
        path
    );
    assert_eq!(
        path.to_string_lossy(),
        "Samples/kick.wav",
        "the empty root element is dropped, the rest is kept"
    );
}

#[test]
fn a_sample_in_the_user_folder_is_found_and_reported_outside_the_project() {
    // The fix has to do more than tidy the path: the sample has to be
    // found, and reported as *outside* the project — collectable, not
    // lost.
    let tmp = std::env::temp_dir().join(format!(
        "clavus_w5_userfolder_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let user_folder = tmp.join("userfolder");
    let project_dir = tmp.join("project");
    fs::create_dir_all(&project_dir).unwrap();

    // The user folder: `<user folder>/Samples/kick.wav`.
    make_sample(&user_folder.join("Samples/kick.wav"), b"FAKE_AUDIO");
    make_als(
        &project_dir.join("Test.als"),
        &live9_user_folder_als("Samples", "kick.wav"),
    );

    // `CLAVUS_USER_FOLDER` is Clavus's own variable, so setting it
    // cannot disturb a test that is reading `HOME` at the same time.
    let previous = std::env::var_os("CLAVUS_USER_FOLDER");
    std::env::set_var("CLAVUS_USER_FOLDER", &user_folder);

    let report = audit_project(&project_dir.join("Test.als"));

    match previous {
        Some(p) => std::env::set_var("CLAVUS_USER_FOLDER", p),
        None => std::env::remove_var("CLAVUS_USER_FOLDER"),
    }

    let report = report.expect("the set should audit");
    assert!(
        report.missing.is_empty(),
        "the sample is in the user folder, not missing: {:?}",
        report.missing.iter().map(|s| &s.path).collect::<Vec<_>>()
    );
    assert_eq!(
        report.outside_project.len(),
        1,
        "it is found, and it is outside the project: {:?}",
        report.outside_project
    );
}

// ─── Live's own factory library is not a missing sample ───────────────────

#[test]
fn a_sample_inside_an_application_bundle_is_live_s_own_content() {
    let path = Path::new(
        "/Applications/Ableton Live 11 Suite.app/Contents/App-Resources/Core Library/Samples/909/Kick 909 Tune11 k.wav",
    );
    assert!(
        is_factory_library_path(path),
        "a sample inside an .app bundle is Live's own"
    );
    assert!(
        !is_factory_library_path(Path::new("/Users/chris/Music/Samples/kick.wav")),
        "a user's own sample is not Live's"
    );
}

#[test]
fn a_factory_sample_does_not_stop_a_snapshot() {
    // 60 of the 200 missing references on a real drive were Live's own
    // factory library. Calling those "missing" said the user's audio
    // was lost, and stopped the snapshot. Nothing was lost: they come
    // back whenever Live is installed.
    let tmp = std::env::temp_dir().join(format!(
        "clavus_w5_factory_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let project_dir = tmp.join("project");
    fs::create_dir_all(&project_dir).unwrap();

    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton>
  <LiveSet>
    <Tracks>
      <AudioTrack>
        <Name Value="Test" />
        <DeviceChain>
          <SampleRef><Path Value="/Applications/Ableton Live 11 Suite.app/Contents/App-Resources/Core Library/Samples/909/Kick 909 Tune11 k.wav"/></SampleRef>
        </DeviceChain>
      </AudioTrack>
    </Tracks>
  </LiveSet>
</Ableton>"#;
    make_als(&project_dir.join("Test.als"), xml);

    let report = audit_project(&project_dir.join("Test.als")).expect("audit");
    assert!(
        report.missing.is_empty(),
        "Live's own sample is not missing: {:?}",
        report.missing
    );
    assert_eq!(report.factory.len(), 1, "it is reported as factory content");
    assert!(
        report.is_clean(),
        "and it does not stop the set being snapshotted"
    );
}

// ─── Live's own backup copies are not projects of their own ───────────────

#[test]
fn a_backup_copy_is_recognised_as_one() {
    // Live keeps a `Backup/` folder and an `Autosave/` folder beside
    // every set, holding copies of it. On a real drive, 2,690 `.als`
    // files were 1,245 sets and 1,445 of Live's own copies — so more
    // than half the list was the same sets again under new names.
    assert!(is_live_backup_copy(Path::new(
        "/Volumes/LaCie/Set/Set Project/Backup/Set [2020-01-30 022324].als"
    )));
    assert!(is_live_backup_copy(Path::new(
        "/Volumes/LaCie/Set/Set Project/Autosave/Set.als"
    )));
    assert!(is_live_backup_copy(Path::new(
        "/Volumes/LaCie/Set/Set Project/backup/Set.als"
    )));
    assert!(
        !is_live_backup_copy(Path::new("/Volumes/LaCie/Set/Set Project/Set.als")),
        "the set itself is not a copy of itself"
    );
    assert!(
        !is_live_backup_copy(Path::new("/Volumes/LaCie/Backups/Set.als")),
        "a folder merely named like one is not Live's own copy"
    );
}

// ─── A project is a pointer, not a copy ───────────────────────────────────

#[test]
fn auditing_a_project_does_not_copy_it_anywhere() {
    // The question a user asks first: does Clavus take a copy of my set
    // into its own store, or does it just remember where the file is?
    // The answer has to be the second one — a library of a thousand sets
    // would otherwise be duplicated in full.
    let tmp = std::env::temp_dir().join(format!(
        "clavus_w5_pointer_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let project_dir = tmp.join("project");
    fs::create_dir_all(&project_dir).unwrap();
    let als = project_dir.join("Test.als");
    make_als(
        &als,
        r#"<?xml version="1.0" encoding="UTF-8"?><LiveSet><Tracks/></LiveSet>"#,
    );
    let before = fs::read(&als).unwrap();

    let _ = audit_project(&als);

    // The set is untouched, and nothing new was written beside it.
    assert_eq!(fs::read(&als).unwrap(), before, "the set is not rewritten");
    let entries: Vec<_> = fs::read_dir(&project_dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(
        entries,
        vec!["Test.als".to_string()],
        "auditing leaves nothing behind"
    );
}

#[test]
fn a_project_outside_the_home_keeps_a_pointer_to_its_own_path() {
    // The identity of a project is its path, so the same set found from
    // two machines is one project, and the store knows where it is.
    let path = PathBuf::from("/Volumes/LaCie/Set/Set Project/Set.als");
    let id = clavus_core::project::ProjectId::new(&path);
    assert_eq!(id.path, path);
    assert!(id.key.contains("Set"), "the key names the set: {}", id.key);
}

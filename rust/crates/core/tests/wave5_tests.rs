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

#[test]
fn live_s_user_library_is_where_a_user_folder_reference_is_measured_from() {
    // A set that says `Samples/kick.wav` means the file in *Live's
    // user library*, which lives at `~/Music/Ableton/User Library` —
    // not at `~/Music/Samples/kick.wav`. Looking in the wrong place
    // made a set that was complete read as broken, and stopped the
    // snapshot for no reason.
    let home = PathBuf::from("/Users/somebody");
    let roots = clavus_core::sample::user_folder_roots_in(
        &home.join("Music/Set/Set Project"),
        Some(&home),
        None,
    );

    assert!(
        roots.contains(&home.join("Music/Ableton/User Library")),
        "Live's user library is a root: {:?}",
        roots
    );
    assert!(
        roots.contains(&home.join("Music")),
        "the music folder is still a root: {:?}",
        roots
    );
}

#[test]
fn live_s_user_library_is_tried_before_the_bare_music_folder() {
    // Order matters: the user library is the *right* answer for a
    // user-folder reference, and `~/Music` is the wider guess. The
    // right one is found first, so a set that has both is not
    // reported as pointing at the wrong file.
    let home = PathBuf::from("/Users/somebody");
    let roots = clavus_core::sample::user_folder_roots_in(
        &home.join("Music/Set/Set Project"),
        Some(&home),
        None,
    );

    let library = roots
        .iter()
        .position(|r| r == &home.join("Music/Ableton/User Library"))
        .expect("the user library is a root");
    let music = roots
        .iter()
        .position(|r| r == &home.join("Music"))
        .expect("the music folder is a root");
    assert!(
        library < music,
        "the user library comes first: {:?}",
        roots
    );
}

#[test]
fn the_user_folder_roots_do_not_depend_on_the_machine_running_the_test() {
    // The home folder is a parameter, so the list is the same on
    // every machine and can be checked without changing the
    // environment.
    let here = PathBuf::from("/Users/somebody");
    let there = PathBuf::from("/Volumes/LaCie/Music");
    let a = clavus_core::sample::user_folder_roots_in(&here, Some(&here), None);
    let b = clavus_core::sample::user_folder_roots_in(&there, Some(&there), None);
    assert_ne!(a, b, "the roots follow the home folder given");
    assert!(a.contains(&here.join("Music/Ableton/User Library")));
    assert!(b.contains(&there.join("Music/Ableton/User Library")));
}

// ─── A set collected on another machine ─────────────────────────────

/// A set that points its samples at another machine, while the audio
/// already sits in the project's own sample folder.
fn foreign_machine_als(sample_name: &str) -> String {
    foreign_machine_als_in("Samples", sample_name)
}

/// The same, with the sample in a folder of the reference's path.
///
/// A collect that flattens the folder leaves the reference pointing at
/// a folder that is no longer there.
fn foreign_machine_als_in(folder: &str, sample_name: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Ableton>
  <LiveSet>
    <Tracks>
      <AudioTrack>
        <Name Value="Test" />
        <DeviceChain>
          <SampleRef><FileRef>
            <Path Value="C:/Users/someone/OneDrive/Desktop/Create/Projects/2025/Test Project/{folder}/{sample_name}" />
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

fn unique_dir(tag: &str) -> PathBuf {
    let tmp = std::env::temp_dir().join(format!(
        "clavus_w5_{}_{}_{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    fs::create_dir_all(&tmp).unwrap();
    tmp
}

#[test]
fn a_reference_to_another_machine_is_pointed_at_the_file_already_in_the_project() {
    // A set collected on a Windows machine keeps the Windows paths
    // even though the audio was brought along. The project therefore
    // audits as missing every sample and refuses to snapshot. The
    // files are right there; only the pointers are wrong.
    let tmp = unique_dir("foreign");
    let project = tmp.join("Test Project");
    fs::create_dir_all(&project).unwrap();
    let als = project.join("Test.als");

    make_sample(&project.join("Samples/loop.wav"), b"FAKE_AUDIO");
    make_als(&als, &foreign_machine_als("loop.wav"));

    let report = audit_project(&als).expect("the set should audit");
    assert_eq!(
        report.missing.len(),
        1,
        "the set points at a machine that is not here: {:?}",
        report.missing
    );

    let collected = clavus_core::sample::collect_samples(&als, &report).expect("collect should run");
    assert!(collected.failed.is_empty(), "nothing failed: {:?}", collected.failed);
    assert_eq!(
        collected.repointed_in_project, 1,
        "one reference was pointed at the file that was there"
    );
    assert!(
        collected.copied.is_empty(),
        "nothing was copied — the file was already in the project"
    );

    // And now the set audits clean.
    let after = audit_project(&als).expect("the set should audit");
    assert!(
        after.missing.is_empty(),
        "the set should no longer be missing anything: {:?}",
        after.missing
    );
}

#[test]
fn two_files_of_the_same_name_in_the_project_are_not_guessed_between() {
    // The repair is only safe when there is one candidate. Two files
    // of the same name is an ambiguity, and picking one would point
    // the set at the wrong audio.
    let tmp = unique_dir("ambiguous");
    let project = tmp.join("Test Project");
    fs::create_dir_all(&project).unwrap();
    let als = project.join("Test.als");

    make_sample(&project.join("Samples/loop.wav"), b"ONE");
    make_sample(&project.join("Samples/Imported/loop.wav"), b"TWO");
    // The reference points at `Samples/Splice/loop.wav`, and the
    // collect flattened that folder away — so only the name can match,
    // and the name matches two different sounds.
    make_als(&als, &foreign_machine_als_in("Samples/Splice", "loop.wav"));

    let report = audit_project(&als).expect("the set should audit");
    let collected = clavus_core::sample::collect_samples(&als, &report).expect("collect should run");

    assert_eq!(
        collected.repointed_in_project, 0,
        "an ambiguous name is not pointed at either file"
    );
    assert_eq!(collected.failed.len(), 1, "the ambiguity is reported as a failure");
}

#[test]
fn a_file_whose_name_holds_a_path_is_found_by_its_last_part() {
    // A collect done on another machine can leave a file whose *name*
    // carries a path separator. It is one file, and it is found by the
    // name the set uses.
    let tmp = unique_dir("mangled");
    let project = tmp.join("Test Project");
    fs::create_dir_all(&project).unwrap();
    let als = project.join("Test.als");

    make_sample(
        &project.join("Samples/Imported/Samples/Samples\\Imported\\loop.wav"),
        b"FAKE_AUDIO",
    );
    make_als(&als, &foreign_machine_als("loop.wav"));

    let report = audit_project(&als).expect("the set should audit");
    let collected = clavus_core::sample::collect_samples(&als, &report).expect("collect should run");

    assert_eq!(
        collected.repointed_in_project, 1,
        "the file whose name holds a path is found: {:?}",
        collected.failed
    );
    let after = audit_project(&als).expect("the set should audit");
    assert!(after.missing.is_empty(), "the set audits clean: {:?}", after.missing);
}

#[test]
fn a_reference_is_matched_by_the_path_it_ends_in_before_its_name() {
    // The reference is an absolute path on another machine, but it
    // ends in the project's own layout. The file it means is the one
    // at that path — not merely a file of the same name.
    let tmp = unique_dir("tail");
    let project = tmp.join("Test Project");
    fs::create_dir_all(&project).unwrap();
    let als = project.join("Test.als");

    // Two files of the same name, in different places, with different
    // audio. Only the one at the referenced path is right.
    make_sample(&project.join("Samples/Imported/loop.wav"), b"THE RIGHT ONE");
    make_sample(&project.join("Samples/loop.wav"), b"A DIFFERENT ONE");
    make_als(&als, &foreign_machine_als("loop.wav"));

    let report = audit_project(&als).expect("the set should audit");
    let collected = clavus_core::sample::collect_samples(&als, &report).expect("collect should run");

    assert_eq!(collected.repointed_in_project, 1, "one reference was repaired");

    let after = audit_project(&als).expect("the set should audit");
    assert!(after.missing.is_empty(), "the set audits clean: {:?}", after.missing);
}

#[test]
fn two_files_of_the_same_name_holding_the_same_audio_are_not_an_ambiguity() {
    // A collect run twice leaves the same audio in two places. Which
    // one the reference ends up pointing at does not matter, because
    // they hold the same bytes.
    let tmp = unique_dir("same");
    let project = tmp.join("Test Project");
    fs::create_dir_all(&project).unwrap();
    let als = project.join("Test.als");

    make_sample(&project.join("Samples/loop.wav"), b"THE AUDIO");
    make_sample(&project.join("Samples/Samples/loop.wav"), b"THE AUDIO");
    make_als(&als, &foreign_machine_als_in("Samples/Splice", "loop.wav"));

    let report = audit_project(&als).expect("the set should audit");
    let collected = clavus_core::sample::collect_samples(&als, &report).expect("collect should run");

    assert_eq!(
        collected.repointed_in_project, 1,
        "the same audio in two places is not a reason to refuse: {:?}",
        collected.failed
    );
    let after = audit_project(&als).expect("the set should audit");
    assert!(after.missing.is_empty(), "the set audits clean: {:?}", after.missing);
}

#[test]
fn two_files_of_the_same_name_holding_different_audio_are_refused() {
    // Two files of the same name that hold *different* audio are a
    // real ambiguity. Picking one would point the set at the wrong
    // sound, so neither is picked.
    let tmp = unique_dir("different");
    let project = tmp.join("Test Project");
    fs::create_dir_all(&project).unwrap();
    let als = project.join("Test.als");

    make_sample(&project.join("Samples/loop.wav"), b"ONE SOUND");
    make_sample(&project.join("Samples/Imported/loop.wav"), b"ANOTHER SOUND");
    make_als(&als, &foreign_machine_als_in("Samples/Splice", "loop.wav"));

    let report = audit_project(&als).expect("the set should audit");
    let collected = clavus_core::sample::collect_samples(&als, &report).expect("collect should run");

    assert_eq!(
        collected.repointed_in_project, 0,
        "different audio under one name is not guessed between"
    );
    assert_eq!(collected.failed.len(), 1, "the ambiguity is reported");
}

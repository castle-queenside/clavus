//! Tests for the project diff.
//!
//! The interesting cases are the ones where a rename looks like a
//! deletion and an addition. A diff that quietly reports a rename
//! when the user deleted a track and added another one is hiding a
//! loss, so those cases are tested hard.

use clavus_core::diff::{diff_projects, diff_sample_refs, TrackStatus};
use clavus_core::types::{Clip, Device, Project, ProjectType, Track};
use std::path::PathBuf;

fn track(name: &str, kind: &str) -> Track {
    Track {
        name: name.into(),
        track_type: kind.into(),
        index: 0,
        color: 0,
        is_frozen: false,
        is_muted: false,
        is_solo: false,
        devices: vec![],
        sends: Default::default(),
        clips: vec![],
    }
}

fn with_device(mut t: Track, device: &str) -> Track {
    t.devices.push(Device {
        name: device.into(),
        device_type: device.into(),
    });
    t
}

fn with_clips(mut t: Track, count: usize) -> Track {
    t.clips = (0..count)
        .map(|i| Clip {
            start_beats: i as f64 * 4.0,
            end_beats: i as f64 * 4.0 + 4.0,
            name: format!("clip {}", i),
            color: 0,
            clip_type: "AudioClip".into(),
        })
        .collect();
    t
}

fn project(tracks: Vec<Track>) -> Project {
    Project {
        path: PathBuf::from("/tmp/set.als"),
        project_type: ProjectType::Ableton,
        name: "set".into(),
        head: None,
        track_count: tracks.len(),
        bpm: 120.0,
        time_signature: "4/4".into(),
        ableton_version: None,
        schema_version: None,
        tracks,
        return_tracks: vec![],
        master_track: None,
        markers: vec![],
        tempo_events: vec![],
    }
}

fn find<'a>(diff: &'a clavus_core::ProjectDiff, name: &str) -> &'a clavus_core::TrackDiff {
    diff.tracks
        .iter()
        .find(|t| t.name == name)
        .unwrap_or_else(|| panic!("no track named {} in the diff: {:?}", name, diff.tracks))
}

#[test]
fn two_identical_projects_have_nothing_to_say() {
    let before = project(vec![track("Bass", "Audio")]);
    let after = project(vec![track("Bass", "Audio")]);
    let diff = diff_projects(&before, &after);
    assert!(diff.is_empty(), "nothing changed: {:?}", diff);
    assert!(diff.changed_tracks().is_empty());
}

#[test]
fn a_tempo_change_is_reported_with_both_numbers() {
    let mut before = project(vec![track("Bass", "Audio")]);
    let mut after = before.clone();
    after.bpm = 140.0;
    before.bpm = 120.0;
    let diff = diff_projects(&before, &after);
    assert_eq!(diff.bpm, Some((120.0, 140.0)));
}

#[test]
fn a_track_count_change_is_reported_with_both_numbers() {
    let before = project(vec![track("Bass", "Audio")]);
    let after = project(vec![track("Bass", "Audio"), track("Keys", "MIDI")]);
    let diff = diff_projects(&before, &after);
    assert_eq!(diff.track_count, Some((1, 2)));
    assert!(diff.has_track_changes());
}

#[test]
fn an_added_track_is_reported_as_added() {
    let before = project(vec![track("Bass", "Audio")]);
    let after = project(vec![track("Bass", "Audio"), track("Keys", "MIDI")]);
    let diff = diff_projects(&before, &after);
    assert_eq!(find(&diff, "Keys").status, TrackStatus::Added);
    assert_eq!(find(&diff, "Bass").status, TrackStatus::Unchanged);
}

#[test]
fn a_removed_track_is_reported_as_removed() {
    let before = project(vec![track("Bass", "Audio"), track("Keys", "MIDI")]);
    let after = project(vec![track("Bass", "Audio")]);
    let diff = diff_projects(&before, &after);
    assert_eq!(find(&diff, "Keys").status, TrackStatus::Removed);
}

#[test]
fn a_renamed_track_is_reported_as_renamed() {
    // Same kind of track, same devices, same clips — only the name
    // changed. This is a rename.
    let before = project(vec![with_clips(
        with_device(track("Bass", "Audio"), "EQ Eight"),
        2,
    )]);
    let after = project(vec![with_clips(
        with_device(track("Sub Bass", "Audio"), "EQ Eight"),
        2,
    )]);
    let diff = diff_projects(&before, &after);
    let renamed = diff
        .tracks
        .iter()
        .find(|t| t.status == TrackStatus::Renamed)
        .expect("the rename should be reported");
    assert_eq!(renamed.name, "Sub Bass");
    assert_eq!(renamed.previous_name.as_deref(), Some("Bass"));
}

#[test]
fn a_deleted_track_and_a_different_added_one_is_not_a_rename() {
    // The user deleted "Bass" and added "Lead". They are the same
    // kind of track, but they do not carry the same devices, so this
    // is a removal and an addition — and it must be reported that
    // way, because "you renamed a track" would hide the deletion.
    let before = project(vec![with_device(track("Bass", "Audio"), "EQ Eight")]);
    let after = project(vec![with_device(track("Lead", "Audio"), "Compressor")]);
    let diff = diff_projects(&before, &after);
    assert!(
        !diff.tracks.iter().any(|t| t.status == TrackStatus::Renamed),
        "this must not be reported as a rename: {:?}",
        diff.tracks
    );
    assert_eq!(find(&diff, "Bass").status, TrackStatus::Removed);
    assert_eq!(find(&diff, "Lead").status, TrackStatus::Added);
}

#[test]
fn an_ambiguous_pair_is_not_guessed_at() {
    // Two tracks removed, two added, and every one of them looks
    // like the others. There is no way to tell which is which, so
    // nothing is claimed to be a rename.
    let before = project(vec![
        with_clips(with_device(track("A", "Audio"), "EQ"), 1),
        with_clips(with_device(track("B", "Audio"), "EQ"), 1),
    ]);
    let after = project(vec![
        with_clips(with_device(track("C", "Audio"), "EQ"), 1),
        with_clips(with_device(track("D", "Audio"), "EQ"), 1),
    ]);
    let diff = diff_projects(&before, &after);
    assert!(
        !diff.tracks.iter().any(|t| t.status == TrackStatus::Renamed),
        "an ambiguous pairing must not be guessed: {:?}",
        diff.tracks
    );
    assert_eq!(
        diff.tracks
            .iter()
            .filter(|t| t.status == TrackStatus::Removed)
            .count(),
        2
    );
    assert_eq!(
        diff.tracks
            .iter()
            .filter(|t| t.status == TrackStatus::Added)
            .count(),
        2
    );
}

#[test]
fn a_track_of_a_different_kind_is_never_a_rename() {
    let before = project(vec![with_clips(track("Bass", "Audio"), 2)]);
    let after = project(vec![with_clips(track("Bass 2", "MIDI"), 2)]);
    let diff = diff_projects(&before, &after);
    assert!(
        !diff.tracks.iter().any(|t| t.status == TrackStatus::Renamed),
        "an audio track is not a MIDI track: {:?}",
        diff.tracks
    );
}

#[test]
fn a_device_added_to_a_track_is_reported() {
    let before = project(vec![track("Bass", "Audio")]);
    let after = project(vec![with_device(track("Bass", "Audio"), "Compressor")]);
    let diff = diff_projects(&before, &after);
    let d = find(&diff, "Bass");
    assert_eq!(d.status, TrackStatus::Modified);
    assert_eq!(d.devices_added, vec!["Compressor".to_string()]);
    assert!(d.devices_removed.is_empty());
}

#[test]
fn a_device_removed_from_a_track_is_reported() {
    let before = project(vec![with_device(track("Bass", "Audio"), "Compressor")]);
    let after = project(vec![track("Bass", "Audio")]);
    let diff = diff_projects(&before, &after);
    let d = find(&diff, "Bass");
    assert_eq!(d.status, TrackStatus::Modified);
    assert_eq!(d.devices_removed, vec!["Compressor".to_string()]);
}

#[test]
fn muting_a_track_is_reported_in_words() {
    let mut before = project(vec![track("Bass", "Audio")]);
    let mut after = before.clone();
    after.tracks[0].is_muted = true;
    before.tracks[0].is_muted = false;
    let diff = diff_projects(&before, &after);
    let d = find(&diff, "Bass");
    assert_eq!(d.status, TrackStatus::Modified);
    assert!(d.details.iter().any(|x| x == "muted"), "{:?}", d.details);
}

#[test]
fn tracks_with_the_same_name_are_matched_in_order() {
    // Ableton allows two tracks with the same name. They are matched
    // in the order they appear, not crashed together.
    let mut before = project(vec![track("Audio 1", "Audio"), track("Audio 1", "Audio")]);
    let mut after = project(vec![track("Audio 1", "Audio"), track("Audio 1", "Audio")]);
    before.tracks[1].is_muted = true;
    after.tracks[1].is_muted = false;
    let diff = diff_projects(&before, &after);
    let changed = diff.tracks.iter().filter(|t| !t.is_unchanged()).count();
    assert_eq!(changed, 1, "exactly one of the pair changed: {:?}", diff.tracks);
    assert!(
        diff.tracks
            .iter()
            .any(|t| t.details.iter().any(|x| x.contains("mut"))),
        "the change should be described: {:?}",
        diff.tracks
    );
}

// ─── Samples ──────────────────────────────────────────────────────────────

fn refs(paths: &[&str]) -> Vec<PathBuf> {
    paths.iter().map(PathBuf::from).collect()
}

#[test]
fn an_added_sample_is_reported() {
    let diff = diff_sample_refs(&refs(&["/a/kick.wav"]), &refs(&["/a/kick.wav", "/a/snare.wav"]));
    assert_eq!(diff.len(), 1);
    assert_eq!(diff[0].status, clavus_core::diff::SampleStatus::Added);
    assert_eq!(diff[0].path, "/a/snare.wav");
}

#[test]
fn a_removed_sample_is_reported() {
    let diff = diff_sample_refs(&refs(&["/a/kick.wav", "/a/snare.wav"]), &refs(&["/a/kick.wav"]));
    assert_eq!(diff.len(), 1);
    assert_eq!(diff[0].status, clavus_core::diff::SampleStatus::Removed);
    assert_eq!(diff[0].path, "/a/snare.wav");
}

#[test]
fn a_sample_at_a_new_path_is_reported_as_moved() {
    // The set was collected into its own folder, so the path it
    // writes down changed even though the file did not.
    let diff = diff_sample_refs(
        &refs(&["/music/Set/Samples/kick.wav"]),
        &refs(&["Samples/kick.wav"]),
    );
    assert_eq!(diff.len(), 1);
    assert_eq!(diff[0].status, clavus_core::diff::SampleStatus::Moved);
    assert_eq!(diff[0].previous_path.as_deref(), Some("/music/Set/Samples/kick.wav"));
    assert_eq!(diff[0].path, "Samples/kick.wav");
}

#[test]
fn the_same_sample_reached_the_same_way_is_not_a_change() {
    let diff = diff_sample_refs(&refs(&["/a/kick.wav"]), &refs(&["/a/kick.wav"]));
    assert!(diff.is_empty(), "nothing changed: {:?}", diff);
}

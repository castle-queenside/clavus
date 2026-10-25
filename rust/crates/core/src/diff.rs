//! What changed between two versions of a project.
//!
//! A snapshot records the bytes of a set, so any two snapshots can be
//! compared by parsing both and looking at the difference. This
//! module holds the comparison itself: it takes two parsed projects
//! and says what is different.
//!
//! The comparison is deliberately conservative about one thing in
//! particular. A track that has been renamed looks, in a list of
//! tracks, exactly like a track that was deleted and a different one
//! that was added. The difference matters — "you renamed a track" and
//! "you lost a track" are very different messages — so a rename is
//! only reported when the evidence is clear:
//!
//! * the two tracks are the same kind of track,
//! * they carry the same devices,
//! * they hold the same number of clips,
//! * and exactly one track on each side could be the other.
//!
//! When any of that is missing, the tracks are reported as one
//! removed and one added. That is the safe way to be wrong: the user
//! is told their track is gone, and they know it is not.

use crate::types::{Project, Track};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// How one track changed between two versions of a project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackStatus {
    /// The track was not there before.
    Added,
    /// The track is not there now.
    Removed,
    /// The track was there before under a different name.
    Renamed,
    /// The track is the same one, with something changed about it.
    Modified,
    /// Nothing about the track changed.
    Unchanged,
}

impl TrackStatus {
    /// A one-word name for the status, for display.
    pub fn as_str(&self) -> &'static str {
        match self {
            TrackStatus::Added => "added",
            TrackStatus::Removed => "removed",
            TrackStatus::Renamed => "renamed",
            TrackStatus::Modified => "modified",
            TrackStatus::Unchanged => "unchanged",
        }
    }
}

/// What happened to one track.
#[derive(Debug, Clone)]
pub struct TrackDiff {
    /// The name of the track after the change.
    ///
    /// For a track that was removed, this is the name it had before.
    pub name: String,
    /// What happened to it.
    pub status: TrackStatus,
    /// The name it had before, when it was renamed.
    pub previous_name: Option<String>,
    /// Devices that are on the track now and were not there before.
    pub devices_added: Vec<String>,
    /// Devices that were on the track before and are not there now.
    pub devices_removed: Vec<String>,
    /// Other things that changed, in words.
    pub details: Vec<String>,
}

impl TrackDiff {
    /// True when nothing about this track changed.
    pub fn is_unchanged(&self) -> bool {
        self.status == TrackStatus::Unchanged
    }
}

/// How one sample reference changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleStatus {
    /// The set does not point at this sample any more.
    Removed,
    /// The set points at this sample now.
    Added,
    /// The same file, reached by a different path.
    Moved,
}

impl SampleStatus {
    /// A one-word name for the status, for display.
    pub fn as_str(&self) -> &'static str {
        match self {
            SampleStatus::Removed => "removed",
            SampleStatus::Added => "added",
            SampleStatus::Moved => "moved",
        }
    }
}

/// What happened to one sample reference.
#[derive(Debug, Clone)]
pub struct SampleDiff {
    /// The path the set points at now.
    ///
    /// For a sample that was removed, this is the path it pointed at
    /// before.
    pub path: String,
    /// What happened to it.
    pub status: SampleStatus,
    /// The path it pointed at before, when it moved.
    pub previous_path: Option<String>,
}

/// Everything that is different between two versions of a project.
#[derive(Debug, Clone, Default)]
pub struct ProjectDiff {
    /// The tempo before and after, when it changed.
    pub bpm: Option<(f64, f64)>,
    /// The number of tracks before and after, when it changed.
    pub track_count: Option<(usize, usize)>,
    /// What happened to each track.
    pub tracks: Vec<TrackDiff>,
    /// What happened to each sample the set points at.
    pub samples: Vec<SampleDiff>,
    /// The message on the snapshot, before and after.
    pub message: Option<(String, String)>,
    /// The notes on the snapshot, before and after.
    pub notes: Option<(String, String)>,
}

impl ProjectDiff {
    /// True when nothing at all changed.
    pub fn is_empty(&self) -> bool {
        self.bpm.is_none()
            && self.track_count.is_none()
            && self.tracks.iter().all(|t| t.is_unchanged())
            && self.samples.is_empty()
            && self.message.is_none()
            && self.notes.is_none()
    }

    /// The tracks that actually changed, in the order they appear.
    pub fn changed_tracks(&self) -> Vec<&TrackDiff> {
        self.tracks
            .iter()
            .filter(|t| !t.is_unchanged())
            .collect()
    }

    /// True when at least one track was added or removed.
    pub fn has_track_changes(&self) -> bool {
        self.tracks.iter().any(|t| {
            matches!(
                t.status,
                TrackStatus::Added | TrackStatus::Removed | TrackStatus::Renamed
            )
        })
    }
}

/// Compare two parsed projects.
///
/// The tracks are matched by name first. A track that kept its name
/// is the same track, whatever else changed about it. Only the tracks
/// left over are considered for a rename, and a rename is only
/// reported when the evidence is clear.
pub fn diff_projects(before: &Project, after: &Project) -> ProjectDiff {
    let mut diff = ProjectDiff::default();

    if (before.bpm - after.bpm).abs() > f64::EPSILON {
        diff.bpm = Some((before.bpm, after.bpm));
    }
    if before.track_count != after.track_count {
        diff.track_count = Some((before.track_count, after.track_count));
    }

    diff.tracks = diff_tracks(&before.tracks, &after.tracks);
    diff
}

/// Compare the sample references two versions of a set point at.
///
/// The paths are the ones the set writes down, so a path that changed
/// because the set was collected into its own folder shows up as a
/// move — which is true, and worth saying.
pub fn diff_sample_refs(before: &[PathBuf], after: &[PathBuf]) -> Vec<SampleDiff> {
    let before_map: HashMap<String, PathBuf> = before
        .iter()
        .map(|p| (path_key(p), p.clone()))
        .collect();
    let after_map: HashMap<String, PathBuf> = after
        .iter()
        .map(|p| (path_key(p), p.clone()))
        .collect();

    let mut out = Vec::new();

    // A path that is on both sides did not move.
    let unchanged: Vec<String> = before_map
        .keys()
        .filter(|k| after_map.contains_key(*k))
        .cloned()
        .collect();

    // A file that is on one side only, and is called the same thing
    // as a file on the other side only, has probably moved. When
    // there is more than one candidate it has not moved as far as
    // anyone can tell, so it is reported as an addition and a
    // removal.
    let mut moved: Vec<(String, String)> = Vec::new();
    for after_key in after_map.keys() {
        if before_map.contains_key(after_key) {
            continue;
        }
        let candidates: Vec<&String> = before_map
            .keys()
            .filter(|before_key| {
                !after_map.contains_key(*before_key) && same_file_name(before_key, after_key)
            })
            .collect();
        if candidates.len() == 1 {
            moved.push((candidates[0].clone(), after_key.clone()));
        }
    }

    // Anything on the after side.
    for (key, path) in &after_map {
        if unchanged.contains(key) {
            continue;
        }
        if let Some((before_key, _)) = moved.iter().find(|(_, a)| a == key) {
            out.push(SampleDiff {
                path: path.to_string_lossy().to_string(),
                status: SampleStatus::Moved,
                previous_path: Some(before_map[before_key].to_string_lossy().to_string()),
            });
            continue;
        }
        out.push(SampleDiff {
            path: path.to_string_lossy().to_string(),
            status: SampleStatus::Added,
            previous_path: None,
        });
    }

    // Anything on the before side.
    for (key, path) in &before_map {
        if after_map.contains_key(key) {
            continue;
        }
        if moved.iter().any(|(b, _)| b == key) {
            continue;
        }
        out.push(SampleDiff {
            path: path.to_string_lossy().to_string(),
            status: SampleStatus::Removed,
            previous_path: None,
        });
    }

    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

/// The key a path is compared by. Two paths that differ only in their
/// separators are the same path.
fn path_key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Whether two paths point at a file with the same name.
fn same_file_name(a: &str, b: &str) -> bool {
    let a = Path::new(a);
    let b = Path::new(b);
    match (a.file_name(), b.file_name()) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// The devices on a track, in a form two tracks can be compared by.
///
/// The same device twice counts twice: a track with two reverbs is
/// not the same track as one with a single reverb.
fn device_signature(track: &Track) -> Vec<String> {
    let mut devices: Vec<String> = track.devices.iter().map(|d| d.name.clone()).collect();
    devices.sort();
    devices
}

/// Compare the tracks of two versions of a project.
fn diff_tracks(before: &[Track], after: &[Track]) -> Vec<TrackDiff> {
    // Tracks with the same name are the same track.
    let mut before_by_name: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, t) in before.iter().enumerate() {
        before_by_name.entry(t.name.as_str()).or_default().push(i);
    }
    let mut after_by_name: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, t) in after.iter().enumerate() {
        after_by_name.entry(t.name.as_str()).or_default().push(i);
    }

    let mut paired: Vec<(usize, usize)> = Vec::new();
    let mut taken_before: Vec<bool> = vec![false; before.len()];
    let mut taken_after: Vec<bool> = vec![false; after.len()];

    // Pair them up name by name. When two tracks share a name, they
    // are paired in the order they appear, which is the order they
    // appear in the set.
    let mut names: Vec<&str> = before_by_name.keys().copied().collect();
    names.sort();
    for name in names {
        let bs = &before_by_name[name];
        let as_ = match after_by_name.get(name) {
            Some(a) => a,
            None => continue,
        };
        for k in 0..bs.len().min(as_.len()) {
            paired.push((bs[k], as_[k]));
            taken_before[bs[k]] = true;
            taken_after[as_[k]] = true;
        }
    }

    let mut out: Vec<TrackDiff> = Vec::new();

    // The tracks that kept their name.
    for (b, a) in &paired {
        out.push(compare_tracks(&before[*b], &after[*a]));
    }

    // The leftovers. One of these might be the other with a new name.
    let leftover_before: Vec<usize> = (0..before.len()).filter(|i| !taken_before[*i]).collect();
    let leftover_after: Vec<usize> = (0..after.len()).filter(|i| !taken_after[*i]).collect();

    let mut renamed_after = vec![false; after.len()];
    let mut removed_before = vec![false; before.len()];

    for &b in &leftover_before {
        // Which of the leftovers could plausibly be this one?
        let candidates: Vec<usize> = leftover_after
            .iter()
            .copied()
            .filter(|a| !renamed_after[*a] && looks_like_the_same_track(&before[b], &after[*a]))
            .collect();

        // Only one candidate, and no other leftover on the other side
        // could also be this one, means the pairing is clear.
        // Anything else is a guess, and a guess here would hide a
        // deletion.
        if candidates.len() == 1 {
            let a = candidates[0];
            let rivals = leftover_before
                .iter()
                .copied()
                .filter(|other| {
                    !removed_before[*other]
                        && *other != b
                        && looks_like_the_same_track(&before[*other], &after[a])
                })
                .count();
            if rivals == 0 {
                renamed_after[a] = true;
                removed_before[b] = true;
                let mut d = compare_tracks(&before[b], &after[a]);
                d.status = TrackStatus::Renamed;
                d.previous_name = Some(before[b].name.clone());
                d.name = after[a].name.clone();
                out.push(d);
                continue;
            }
        }
    }

    for &b in &leftover_before {
        if removed_before[b] {
            continue;
        }
        out.push(TrackDiff {
            name: before[b].name.clone(),
            status: TrackStatus::Removed,
            previous_name: None,
            devices_added: vec![],
            devices_removed: vec![],
            details: vec![],
        });
    }
    for &a in &leftover_after {
        if renamed_after[a] {
            continue;
        }
        out.push(TrackDiff {
            name: after[a].name.clone(),
            status: TrackStatus::Added,
            previous_name: None,
            devices_added: vec![],
            devices_removed: vec![],
            details: vec![],
        });
    }

    // Keep the order of the set, so the list reads like the set.
    out.sort_by(|x, y| {
        let xa = after
            .iter()
            .position(|t| t.name == x.name)
            .unwrap_or(usize::MAX);
        let ya = after
            .iter()
            .position(|t| t.name == y.name)
            .unwrap_or(usize::MAX);
        xa.cmp(&ya)
    });
    out
}

/// Whether two tracks could be the same track under a different name.
///
/// The evidence has to be strong. The same kind of track is not
/// enough — every project has several of the same kind. The same
/// devices on the same kind of track, holding the same number of
/// clips, is much harder to explain any other way.
fn looks_like_the_same_track(before: &Track, after: &Track) -> bool {
    before.track_type == after.track_type
        && device_signature(before) == device_signature(after)
        && before.clips.len() == after.clips.len()
}

/// Compare two tracks that are known to be the same track.
fn compare_tracks(before: &Track, after: &Track) -> TrackDiff {
    let mut devices_added = vec![];
    let mut devices_removed = vec![];

    let before_devices = device_signature(before);
    let after_devices = device_signature(after);

    for device in &after_devices {
        if !before_devices.contains(device) {
            devices_added.push(device.clone());
        }
    }
    for device in &before_devices {
        if !after_devices.contains(device) {
            devices_removed.push(device.clone());
        }
    }

    let mut details = vec![];
    if before.is_muted != after.is_muted {
        details.push(if after.is_muted {
            "muted".to_string()
        } else {
            "unmuted".to_string()
        });
    }
    if before.is_solo != after.is_solo {
        details.push(if after.is_solo {
            "soloed".to_string()
        } else {
            "unsoloed".to_string()
        });
    }
    if before.is_frozen != after.is_frozen {
        details.push(if after.is_frozen {
            "frozen".to_string()
        } else {
            "unfrozen".to_string()
        });
    }
    if before.clips.len() != after.clips.len() {
        details.push(format!(
            "{} clips → {} clips",
            before.clips.len(),
            after.clips.len()
        ));
    }

    let status = if devices_added.is_empty()
        && devices_removed.is_empty()
        && details.is_empty()
        && before.name == after.name
    {
        TrackStatus::Unchanged
    } else {
        TrackStatus::Modified
    };

    TrackDiff {
        name: after.name.clone(),
        status,
        previous_name: None,
        devices_added,
        devices_removed,
        details,
    }
}

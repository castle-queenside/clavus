//! Project identity — how Clavus tells one project from another.
//!
//! Two projects that happen to share a file name are *not* the same
//! project. `Album1/Track.als` and `Album2/Track.als` are two different
//! sets with two different histories, and a snapshot of one must never be
//! written over the other.
//!
//! Clavus therefore keeps two names for a project:
//!
//! - **the storage key** — derived from the project's *whole path*
//!   (`Track.als__a1b2c3d4e5f6`). It is used for the project's HEAD, its
//!   snapshot records and its cues, so two projects never share a chain.
//!   Two projects in folders of the same name — `p1/studio/Song.als` and
//!   `p2/studio/Song.als` — are two projects, and the key says so.
//! - **the sync name** — the file name with its extension (`Track.als`).
//!   It is used on the wire, because two machines working on the same
//!   project have it at different paths and must still agree on it.
//! - **the folder path** — the project's folder, relative to the home
//!   directory (`p1/studio`). It is the second half of a project's
//!   identity on the wire: the file name says *which* project it is, and
//!   the folder says *which one* when two of them share a file name.

use std::path::{Path, PathBuf};

/// The identity of a DAW project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectId {
    /// Storage key: the project folder and file name, made safe for use
    /// as a directory name.
    pub key: String,
    /// Display name: the file's stem, e.g. `Track`.
    pub name: String,
    /// The file name with its extension, e.g. `Track.als`.
    pub file_name: String,
    /// The name of the folder the project lives in, e.g. `Album1`.
    ///
    /// It is empty when the project sits at the top of a directory.
    pub folder: String,
    /// The project's folder, relative to the home directory,
    /// e.g. `Album1/Session2`.
    ///
    /// It is the folder's *path*, not just its name: two projects in
    /// folders of the same name are two projects.
    pub folder_path: String,
    /// The project's path.
    pub path: PathBuf,
}

impl ProjectId {
    /// Work out the identity of the project at `path`.
    pub fn new(path: &Path) -> Self {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        Self::new_in(path, home.as_deref())
    }

    /// Work out the identity of the project at `path`, with the path
    /// made absolute first.
    ///
    /// A project given as a relative path is the same project as the
    /// same file named by its full path, and it must have the same
    /// identity: `notes/p1/studio/Song.als` and
    /// `/home/chris/notes/p1/studio/Song.als` are one project, not two.
    pub fn new_absolute(path: &Path) -> Self {
        Self::new(&absolute_path(path))
    }

    /// Work out the identity of the project at `path`, given the home
    /// directory of the machine.
    ///
    /// The home directory is the root of the folder path, so a test can
    /// say what it means without changing the machine it runs on.
    pub fn new_in(path: &Path, home: Option<&Path>) -> Self {
        let path = &absolute_path(path);
        let file_name = path
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "project".to_string());

        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "project".to_string());

        // The project folder is the last component of the parent path.
        let folder = path
            .parent()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().to_string())
            .filter(|s| !s.is_empty() && s != "." && s != "..");

        // The key must name *this* project and no other. The file name
        // and the name of the folder are not enough: `p1/studio/Song.als`
        // and `p2/studio/Song.als` share both, and they are two
        // different sets. A digest of the whole path tells them apart.
        let mut base = sanitize(&file_name);
        // A key becomes a directory name, so it must not begin with
        // dots: `..als` would name a parent directory.
        if base.starts_with('.') {
            base = format!("p-{}", base);
        }
        let key = format!(
            "{}__{}",
            base,
            &crate::blob::BlobStore::sha256(path.to_string_lossy().as_bytes())[..12]
        );

        Self {
            key,
            name,
            file_name,
            folder: folder.unwrap_or_default(),
            folder_path: folder_path_of(path, home),
            path: path.to_path_buf(),
        }
    }

    /// The name this project is known by on the sync wire.
    ///
    /// It is the file name with its extension, so an Ableton set and a
    /// Bitwig bundle that happen to share a stem stay apart, while two
    /// machines holding the same project still agree on the name.
    pub fn sync_name(&self) -> &str {
        &self.file_name
    }

    /// The name of the folder the project lives in.
    ///
    /// This is the second half of the project's identity on the wire.
    /// Two projects that share a file name but live in different
    /// folders are different projects, and a relay that only knew the
    /// file name would hand one's history to the other.
    pub fn folder(&self) -> &str {
        &self.folder
    }

    /// The project's folder path, relative to the home directory.
    pub fn folder_path(&self) -> &str {
        &self.folder_path
    }

    /// The project's identity on the relay.
    ///
    /// The file name says *which* project it is; the folder path says
    /// *which one* when two of them share a file name.
    pub fn relay_key(&self) -> String {
        relay_key_for(&self.file_name, &self.folder_path)
    }
}

/// The relay's key for a project: its file name and the folder it lives
/// in.
///
/// The two parts are separated by `::`, so a colon inside either of
/// them is escaped. A project called `A.als::B` in the folder `C` and a
/// project called `A.als` in the folder `B::C` are two projects, and
/// their keys must not be the same string — one project's history
/// would block the other's push and the relay would serve one
/// project's notes to a client working on the other.
pub fn relay_key_for(file_name: &str, folder: &str) -> String {
    format!(
        "{}::{}",
        escape_key_part(file_name),
        escape_key_part(folder)
    )
}

/// Escape one part of a relay key so it cannot be mistaken for the
/// separator.
pub fn escape_key_part(part: &str) -> String {
    part.replace(':', "%3A")
}

/// Undo [`escape_key_part`].
pub fn unescape_key_part(part: &str) -> String {
    part.replace("%3A", ":")
}

/// Whether two folder paths name two places in the same tree.
///
/// `notes/sep/p1/studio` and `notes/sep/p2/studio` are two folders with
/// the same name in different places: two projects a user keeps side by
/// side. `Music/Album` and `Work/Album` are two differently named
/// folders, which is what a collaborator's copy usually looks like.
///
/// The relay uses this to tell a collaborator on another machine from
/// the other project next door. Two projects that sit in the same tree
/// are not the same project, however identical their bytes are, and
/// serving one the other's history is a leak.
///
/// The two folders must also be at the same depth. A folder of the
/// same name at a different depth is not "the project next door" — it
/// is a copy in a different place, which is what a collaborator has.
/// `Studio` under the home directory and `Volumes/Studio` on an
/// external disk are one project seen from two machines, and the
/// bytes of the file are what prove it.
pub fn folders_are_siblings(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() || a == b {
        return false;
    }
    fn leaf(p: &str) -> &str {
        p.rsplit('/').next().unwrap_or(p)
    }
    fn parent(p: &str) -> String {
        let mut parts: Vec<&str> = p.split('/').collect();
        parts.pop();
        parts.join("/")
    }
    fn depth(p: &str) -> usize {
        p.split('/').count()
    }
    // The same folder name in a different place. Two projects of the
    // same name, kept side by side.
    leaf(a) == leaf(b) && parent(a) != parent(b) && depth(a) == depth(b)
}

/// The path made absolute, with `.` and `..` resolved.
///
/// The path is not resolved through symlinks: a project's identity
/// must not change because the machine happens to mount a folder
/// differently today than yesterday.
pub fn absolute_path(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut out = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                // A `..` that would climb above the root is dropped:
                // the root is the end of the road.
                if !out.pop() {
                    out.push("/");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

/// The folder a project lives in, relative to the home directory.
///
/// Two machines that keep the same tree agree on it, and it does not
/// carry the user's name. It is the folder's *path*, not just its
/// name: `p1/studio` and `p2/studio` are two folders, and two
/// projects that live in them are two projects.
///
/// A project outside the home directory keeps the name of the folder
/// it lives in. That is the one part of the path two machines can be
/// expected to agree on — a path that named the machine would break
/// the collaboration between them, and the relay tells two projects of
/// the same name apart by their bytes and by their history.
pub fn folder_path_of(path: &Path, home: Option<&Path>) -> String {
    let Some(parent) = path.parent() else {
        return ".".to_string();
    };
    // Under the home directory the folder path is the path below it.
    //
    // The home directory itself is sometimes reached through a symlink
    // — a home on another disk, a temporary directory, a mounted
    // volume — and then the paths do not match even though the project
    // is plainly inside the home. The comparison is tried a second time
    // with both paths resolved, so the folder path stays the same
    // wherever the home happens to live.
    let relative: Option<Vec<String>> = home
        .and_then(|home| parent.strip_prefix(home).ok())
        .map(path_parts)
        .or_else(|| {
            let home = std::fs::canonicalize(home?).ok()?;
            let canonical_parent = std::fs::canonicalize(parent).ok()?;
            let relative = canonical_parent.strip_prefix(&home).ok()?;
            Some(path_parts(relative))
        });
    let mut parts: Vec<String> = match relative {
        Some(parts) => parts,
        None => {
            // A project outside the home directory keeps the last two
            // folders of the folder it lives in.
            //
            // The name of the folder on its own was not enough: two
            // projects that live in `p1/studio` and `p2/studio` on a
            // disk outside the home both kept the name "studio", so
            // they had one identity between them, and the relay served
            // one the other's history and notes.
            //
            // Two folders are still a path two machines can be expected
            // to agree on: it does not name the machine, and it does
            // not change because a disk is mounted somewhere else.
            let all = path_parts(parent);
            let mut tail: Vec<String> = all.iter().rev().take(2).rev().cloned().collect();
            if tail.is_empty() {
                if let Some(name) = parent.file_name() {
                    let name = name.to_string_lossy().to_string();
                    if !name.is_empty() && name != "." && name != ".." {
                        tail.push(name);
                    }
                }
            }
            tail
        }
    };
    if parts.is_empty() {
        // The project sits at the top of a directory.
        parts.push(".".to_string());
    }
    parts.join("/")
}

/// The normal components of a path, as plain names.
fn path_parts(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => Some(s.to_string_lossy().to_string()),
            _ => None,
        })
        .collect()
}

/// Make a string safe to use as a single path component.
///
/// Everything outside `[A-Za-z0-9._-]` becomes a dash, so a project called
/// `My Track (final).als` still yields a usable key. The result never
/// escapes its directory and is never a relative path element.
pub fn sanitize(input: &str) -> String {
    let mut out: String = input
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();

    if out.is_empty() || out == "." || out == ".." {
        out = "project".to_string();
    }

    // A key becomes a directory name, so it must stay a sane length.
    if out.len() > 100 {
        let digest = crate::blob::BlobStore::sha256(input.as_bytes());
        out = format!("{}-{}", &out[..80], &digest[..8]);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_projects_with_the_same_file_name_get_different_keys() {
        let a = ProjectId::new(Path::new("/music/Album1/Track.als"));
        let b = ProjectId::new(Path::new("/music/Album2/Track.als"));
        assert_ne!(a.key, b.key, "two projects must never share a key");
        assert_eq!(a.name, b.name, "the display name is still the stem");
    }

    #[test]
    fn an_als_and_a_bwproject_with_the_same_stem_are_different_projects() {
        let a = ProjectId::new(Path::new("/music/Set/Set.als"));
        let b = ProjectId::new(Path::new("/music/Set/Set.bwproject"));
        assert_ne!(a.key, b.key);
    }

    #[test]
    fn the_sync_name_is_the_file_name() {
        let p = ProjectId::new(Path::new("/music/Album1/Track.als"));
        assert_eq!(p.sync_name(), "Track.als");
    }

    #[test]
    fn a_key_is_always_a_safe_path_component() {
        for name in [
            "/music/My Track (final)/My Track (final).als",
            "/music/../../etc/passwd",
            "/music/a/b/c/..als",
            "/music/ünïcödé/ próject.als",
        ] {
            let key = ProjectId::new(Path::new(name)).key;
            assert!(
                !key.contains('/') && !key.contains('\\'),
                "a key must not contain a separator: {}",
                key
            );
            assert!(key != "." && key != "..", "a key must not be relative");
            assert!(
                !key.starts_with(".."),
                "a key must not start with dots: {}",
                key
            );
        }
    }

    #[test]
    fn a_bare_file_name_still_gets_a_key() {
        let p = ProjectId::new(Path::new("demo.als"));
        assert!(p.key.starts_with("demo.als__"), "the key: {}", p.key);
        assert_eq!(p.name, "demo");
    }

    #[test]
    fn two_projects_in_folders_of_the_same_name_get_different_keys() {
        // The folder's *name* is not enough: these two share both the
        // folder name and the file name.
        let a = ProjectId::new(Path::new("/music/p1/studio/Song.als"));
        let b = ProjectId::new(Path::new("/music/p2/studio/Song.als"));
        assert_ne!(a.key, b.key, "two projects must never share a key");
    }

    #[test]
    fn the_folder_path_tells_two_projects_apart() {
        // Under the home directory the folder path is the whole path
        // below it, so two folders of the same name stay apart.
        let home = Path::new("/home/chris");
        let a = ProjectId::new_in(Path::new("/home/chris/music/p1/studio/Song.als"), Some(home));
        let b = ProjectId::new_in(Path::new("/home/chris/music/p2/studio/Song.als"), Some(home));
        assert_ne!(a.folder_path(), b.folder_path());
        assert_ne!(a.relay_key(), b.relay_key());
        assert_eq!(a.folder_path(), "music/p1/studio");
        assert_eq!(a.folder(), "studio", "the name is still the last part");
    }

    #[test]
    fn a_colon_in_a_name_cannot_forge_another_project() {
        // "A.als::B" in the folder "C" and "A.als" in the folder "B::C"
        // are two projects, and their keys must differ.
        let a = relay_key_for("A.als::B", "C");
        let b = relay_key_for("A.als", "B::C");
        assert_ne!(a, b, "two projects must never share a key");
        assert_eq!(unescape_key_part(a.split("::").next().unwrap()), "A.als::B");
    }

    #[test]
    fn a_project_outside_the_home_keeps_its_folder_name() {
        // Two machines must agree on the folder of a project they
        // share, so the path of the machine is not part of it.
        let home = Path::new("/home/chris");
        let a = ProjectId::new_in(Path::new("/media/studio/Song.als"), Some(home));
        assert_eq!(a.folder_path(), "media/studio");
        assert_eq!(a.relay_key(), "Song.als::media/studio");
    }

    #[test]
    fn two_projects_in_folders_of_the_same_name_outside_the_home_stay_apart() {
        // Two copies of a set on an external disk, kept side by side.
        // They shared one identity when the folder path was only the
        // name of the folder, and the relay served one the other's
        // history and notes.
        let home = Path::new("/home/chris");
        let a = ProjectId::new_in(Path::new("/media/p1/studio/Song.als"), Some(home));
        let b = ProjectId::new_in(Path::new("/media/p2/studio/Song.als"), Some(home));
        assert_ne!(a.folder_path(), b.folder_path());
        assert_ne!(a.relay_key(), b.relay_key());
    }

    #[test]
    fn a_copy_on_another_disk_still_agrees_with_a_copy_in_the_home() {
        // A collaborator's copy of a set. The two machines keep it in
        // different places — one under the home directory, one on an
        // external disk — and the bytes of the file are what prove
        // they are the same project.
        let home = Path::new("/home/chris");
        let a = ProjectId::new_in(Path::new("/home/chris/Studio/Song.als"), Some(home));
        let b = ProjectId::new_in(Path::new("/Volumes/Studio/Song.als"), Some(home));
        assert!(
            !folders_are_siblings(a.folder_path(), b.folder_path()),
            "a copy at a different depth is not the project next door"
        );
    }

    #[test]
    fn the_project_next_door_is_a_sibling() {
        assert!(folders_are_siblings("p1/studio", "p2/studio"));
        assert!(folders_are_siblings("media/p1/studio", "media/p2/studio"));
        assert!(!folders_are_siblings("p1/studio", "p1/studio"));
        assert!(!folders_are_siblings("p1/studio", "music/album"));
        assert!(!folders_are_siblings("", "p1/studio"));
    }
}

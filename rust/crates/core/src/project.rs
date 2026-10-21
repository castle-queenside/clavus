//! Project identity — how Clavus tells one project from another.
//!
//! Two projects that happen to share a file name are *not* the same
//! project. `Album1/Track.als` and `Album2/Track.als` are two different
//! sets with two different histories, and a snapshot of one must never be
//! written over the other.
//!
//! Clavus therefore keeps two names for a project:
//!
//! - **the storage key** — derived from the project's *folder* as well as
//!   its file name (`Album1__Track.als`). It is used for the project's
//!   HEAD, its snapshot records and its cues, so two projects never share
//!   a chain.
//! - **the sync name** — the file name with its extension (`Track.als`).
//!   It is used on the wire, because two machines working on the same
//!   project have it at different paths and must still agree on it.

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
    /// The project's path.
    pub path: PathBuf,
}

impl ProjectId {
    /// Work out the identity of the project at `path`.
    pub fn new(path: &Path) -> Self {
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

        let key = match folder {
            Some(f) => format!("{}__{}", sanitize(&f), sanitize(&file_name)),
            None => sanitize(&file_name),
        };

        Self {
            key,
            name,
            file_name,
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
        assert_eq!(p.key, "demo.als");
        assert_eq!(p.name, "demo");
    }
}

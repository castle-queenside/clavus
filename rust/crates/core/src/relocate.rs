//! Relocation map — where a path a set points at has moved to.
//!
//! A Live set records the absolute path of every sample it uses. When a
//! drive is renamed, a machine is rebuilt under a different user name, or
//! a library moves to another disk, every one of those paths stops
//! resolving and the set looks like it has lost its audio.
//!
//! Clavus does not guess where the audio went. It keeps a list the user
//! wrote — "this old root is this new root" — and the audit asks that
//! list before it calls a sample missing. A mapping that is wrong is
//! visible in one line of a file; a search across a terabyte of disk is
//! not.
//!
//! The file lives beside the rest of Clavus's state:
//!
//! ```text
//! ~/.clavus/relocations.json
//! ```

use crate::types::ClavusError;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Component, Path, PathBuf};
use tracing::debug;

/// One "this moved there" rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relocation {
    /// The path the set points at, as it was written down.
    pub from: String,
    /// Where that path lives now.
    pub to: String,
}

/// The user's relocation rules, longest match first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relocations {
    #[serde(default)]
    pub entries: Vec<Relocation>,
}

impl Relocations {
    /// Read the map from a Clavus store.
    ///
    /// A store with no map is not an error — most machines never need
    /// one. A map that exists but cannot be read *is* reported, because
    /// silently treating a broken map as an empty one would report the
    /// user's audio as lost.
    pub fn load(clavus_root: &Path) -> Result<Relocations, ClavusError> {
        let path = clavus_root.join("relocations.json");
        match fs::read(&path) {
            Ok(bytes) => {
                let mut map: Relocations = serde_json::from_slice(&bytes)
                    .map_err(|e| ClavusError::Config(format!("{}: {}", path.display(), e)))?;
                debug!("loaded {} relocation rule(s)", map.entries.len());
                map.sort();
                Ok(map)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Relocations::default()),
            Err(e) => Err(ClavusError::Config(format!("{}: {}", path.display(), e))),
        }
    }

    /// Write the map, atomically.
    pub fn save(&self, clavus_root: &Path) -> Result<(), ClavusError> {
        fs::create_dir_all(clavus_root)?;
        let path = clavus_root.join("relocations.json");
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|e| ClavusError::Config(format!("relocations: {}", e)))?;
        crate::blob::atomic_write(&path, &bytes)
            .map_err(|e| ClavusError::Config(format!("{}: {}", path.display(), e)))
    }

    /// Add a rule, replacing any rule with the same `from`.
    pub fn map(&mut self, from: &str, to: &str) -> Result<(), ClavusError> {
        let from = from.trim_end_matches('/');
        let to = to.trim_end_matches('/');
        if from.is_empty() || to.is_empty() {
            return Err(ClavusError::Config(
                "a relocation needs both a path and where it went".into(),
            ));
        }
        if from == to {
            return Err(ClavusError::Config(
                "that path already points at itself".into(),
            ));
        }
        self.entries.retain(|e| e.from != from);
        self.entries.push(Relocation {
            from: from.to_string(),
            to: to.to_string(),
        });
        self.sort();
        Ok(())
    }

    /// Remove a rule. Returns true when one was there.
    pub fn unmap(&mut self, from: &str) -> bool {
        let from = from.trim_end_matches('/');
        let before = self.entries.len();
        self.entries.retain(|e| e.from != from);
        before != self.entries.len()
    }

    /// The longest rule first, so the most specific one wins.
    fn sort(&mut self) {
        self.entries.sort_by(|a, b| {
            b.from
                .len()
                .cmp(&a.from.len())
                .then_with(|| a.from.cmp(&b.from))
        });
    }

    /// Where a reference could have moved to.
    ///
    /// A rule only applies on a path-component boundary: `/Volumes/Set`
    /// does not match `/Volumes/Set Two`. Without that, one rule would
    /// quietly rewrite another folder's paths.
    pub fn candidates(&self, reference: &Path) -> Vec<PathBuf> {
        let reference = reference.to_string_lossy();
        let mut out = Vec::new();
        for entry in &self.entries {
            let rest = match strip_prefix_component(&reference, &entry.from) {
                Some(rest) => rest,
                None => continue,
            };
            let mut candidate = PathBuf::from(&entry.to);
            if !rest.is_empty() {
                candidate.push(rest.trim_start_matches('/'));
            }
            if !out.contains(&candidate) {
                out.push(candidate);
            }
        }
        out
    }
}

/// The part of `path` after `prefix`, when `path` starts with `prefix`.
///
/// The prefix has to end on a component boundary, or `/Volumes/Set`
/// would match `/Volumes/Set Two/file.wav`.
fn strip_prefix_component<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    let prefix = prefix.trim_end_matches('/');
    if prefix.is_empty() {
        return None;
    }
    let rest = path.strip_prefix(prefix)?;
    if rest.is_empty() {
        return Some(rest);
    }
    // The prefix has to end where a path component ends.
    if rest.starts_with('/') {
        Some(rest)
    } else {
        None
    }
}

/// Is this path safe to use as a relocation root?
///
/// A root that climbs out of its own directory with `..` would let a
/// rule rewrite a reference into somewhere the user never meant.
pub fn is_safe_root(path: &str) -> bool {
    let p = Path::new(path);
    if !p.is_absolute() {
        return false;
    }
    !p.components().any(|c| matches!(c, Component::ParentDir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rule_matches_on_a_component_boundary() {
        let map = Relocations {
            entries: vec![Relocation {
                from: "/Volumes/Set".into(),
                to: "/Volumes/LaCie".into(),
            }],
        };
        assert_eq!(
            map.candidates(Path::new("/Volumes/Set/a.wav")),
            vec![PathBuf::from("/Volumes/LaCie/a.wav")]
        );
        // "Set Two" is a different folder and must not be rewritten.
        assert!(map
            .candidates(Path::new("/Volumes/Set Two/a.wav"))
            .is_empty());
    }

    #[test]
    fn the_longest_rule_is_tried_first() {
        let mut map = Relocations {
            entries: vec![
                Relocation {
                    from: "/Users/old".into(),
                    to: "/Users/new".into(),
                },
                Relocation {
                    from: "/Users/old/Music".into(),
                    to: "/Volumes/Music".into(),
                },
            ],
        };
        map.sort();
        // Both rules are the user's own statements and both are kept,
        // but the more specific one is tried first.
        assert_eq!(
            map.candidates(Path::new("/Users/old/Music/a.wav")),
            vec![
                PathBuf::from("/Volumes/Music/a.wav"),
                PathBuf::from("/Users/new/Music/a.wav"),
            ]
        );
    }
}

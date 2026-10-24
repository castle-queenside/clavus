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

/// Is this path safe to use as the *destination* of a relocation?
///
/// The destination has to be an absolute path on this machine, and it
/// cannot climb out of its own folder with `..` — a rule that rewrote a
/// reference into somewhere the user never meant would be worse than no
/// rule at all.
pub fn is_safe_target(path: &str) -> bool {
    let p = Path::new(path);
    if !p.is_absolute() {
        return false;
    }
    !p.components().any(|c| matches!(c, Component::ParentDir))
}

/// Is this path safe to use as the *start* of a relocation?
///
/// The start is matched against the reference exactly as the set wrote
/// it, so it may be absolute, relative, or a Windows path from a set
/// that was made on a PC. It only has to be free of `..`.
pub fn is_safe_source(path: &str) -> bool {
    !Path::new(path)
        .components()
        .any(|c| matches!(c, Component::ParentDir))
}

/// A rule that would put a group of missing samples back where they are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleSuggestion {
    /// The path the sets point at.
    pub from: String,
    /// Where those paths live now.
    pub to: String,
    /// How many distinct references this rule would recover.
    pub references: usize,
}

/// What a search of the library turned up.
#[derive(Debug, Clone, Default)]
pub struct Suggestions {
    /// The rules, most valuable first.
    pub rules: Vec<RuleSuggestion>,
    /// References with more than one equally good home. They are named
    /// so, and no rule is proposed for them.
    pub ambiguous: usize,
    /// References with no file of that name anywhere under the root.
    pub unmatched: usize,
}

/// Propose rules for the references that no longer resolve.
///
/// The library is walked once and indexed by file name. A reference that
/// has no file of that name under the root is simply gone — there is
/// nothing to propose. A reference with several files of that name is
/// ambiguous, and no rule is proposed for it either: serving the wrong
/// copy of the same audio is worse than serving none.
///
/// Everything else proposes one rule: the part of the reference that
/// changed, and the part of the file's real path that replaces it.
/// Nothing is applied — the user decides.
pub fn suggest_rules(references: &[PathBuf], root: &Path) -> Suggestions {
    let index = index_by_name(root);
    let mut counts: std::collections::HashMap<(String, String), usize> =
        std::collections::HashMap::new();
    let mut ambiguous = 0usize;
    let mut unmatched = 0usize;

    for reference in references {
        let name = match reference.file_name() {
            Some(n) => n.to_string_lossy().to_string(),
            None => continue,
        };
        let candidates = match index.get(&name) {
            Some(c) if !c.is_empty() => c,
            _ => {
                unmatched += 1;
                continue;
            }
        };

        // The candidate whose tail matches the reference's tail best is
        // the one that is probably the right copy.
        let mut best_overlap = 0usize;
        let mut best: Vec<&PathBuf> = Vec::new();
        for candidate in candidates {
            let overlap = suffix_overlap(reference, candidate);
            if overlap > best_overlap {
                best_overlap = overlap;
                best.clear();
                best.push(candidate);
            } else if overlap == best_overlap && best_overlap > 0 {
                best.push(candidate);
            }
        }
        if best_overlap == 0 {
            ambiguous += 1;
            continue;
        }
        // Several copies of the same file of that name are not a real
        // choice: whichever one is picked, the audio is the same. Only
        // copies that differ are ambiguous.
        if best.len() > 1 && !all_identical(&best) {
            ambiguous += 1;
            continue;
        }
        let candidate = best[0];

        let mut drop = best_overlap;
        let from = match drop_last(reference, drop) {
            Some(f) => f,
            None => {
                // The whole reference matched the tail of the file, so
                // there is nothing left for the rule to start from. A
                // rule still has to start somewhere: leave the last part
                // of the reference to the destination.
                if drop == 0 {
                    continue;
                }
                drop -= 1;
                match drop_last(reference, drop) {
                    Some(f) => f,
                    None => continue,
                }
            }
        };
        let to = match drop_last(candidate, drop) {
            Some(t) => t,
            None => continue,
        };
        if from.is_empty() || to.is_empty() {
            continue;
        }
        *counts.entry((from, to)).or_insert(0) += 1;
    }

    let mut rules: Vec<RuleSuggestion> = counts
        .into_iter()
        .map(|((from, to), references)| RuleSuggestion {
            from,
            to,
            references,
        })
        .collect();
    rules.sort_by(|a, b| {
        b.references
            .cmp(&a.references)
            .then_with(|| a.from.cmp(&b.from))
    });
    Suggestions {
        rules,
        ambiguous,
        unmatched,
    }
}

/// Every file under `root`, indexed by its name.
///
/// A name that appears thousands of times is not useful to index — the
/// first few hundred are enough to see that it is ambiguous — and
/// keeping them all would cost more memory than the answer is worth.
fn index_by_name(root: &Path) -> std::collections::HashMap<String, Vec<PathBuf>> {
    const PER_NAME_LIMIT: usize = 64;
    let mut index: std::collections::HashMap<String, Vec<PathBuf>> =
        std::collections::HashMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let file_type = match entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if file_type.is_dir() {
                // A link back up the tree would make this walk forever.
                if file_type.is_symlink() {
                    continue;
                }
                stack.push(path);
                continue;
            }
            let name = match path.file_name() {
                Some(n) => n.to_string_lossy().to_string(),
                None => continue,
            };
            let bucket = index.entry(name).or_default();
            if bucket.len() < PER_NAME_LIMIT {
                bucket.push(path);
            }
        }
    }
    index
}

/// How many of the last path components the two paths share.
fn suffix_overlap(a: &Path, b: &Path) -> usize {
    let pa: Vec<_> = a.components().collect();
    let pb: Vec<_> = b.components().collect();
    let mut k = 0;
    while k < pa.len() && k < pb.len() && pa[pa.len() - 1 - k] == pb[pb.len() - 1 - k] {
        k += 1;
    }
    k
}

/// The path with its last `drop` components removed.
///
/// The components are the path's own, so a trailing slash or a doubled
/// separator cannot make this and [`suffix_overlap`] disagree about
/// what a component is.
fn drop_last(path: &Path, drop: usize) -> Option<String> {
    let components: Vec<_> = path.components().collect();
    if drop >= components.len() {
        return None;
    }
    let kept: PathBuf = components[..components.len() - drop].iter().collect();
    let text = kept.to_string_lossy().to_string();
    if text.is_empty() {
        return None;
    }
    Some(text)
}

/// Do all of these files hold exactly the same bytes?
fn all_identical(candidates: &[&PathBuf]) -> bool {
    let first = match candidates.first() {
        Some(f) => *f,
        None => return false,
    };
    let size = match std::fs::metadata(first).map(|m| m.len()) {
        Ok(s) => s,
        Err(_) => return false,
    };
    for other in &candidates[1..] {
        if std::fs::metadata(other).map(|m| m.len()).ok() != Some(size) {
            return false;
        }
        if !same_bytes(first, other) {
            return false;
        }
    }
    true
}

/// Do two files hold exactly the same bytes?
fn same_bytes(a: &Path, b: &Path) -> bool {
    let (mut fa, mut fb) = match (std::fs::File::open(a), std::fs::File::open(b)) {
        (Ok(x), Ok(y)) => (x, y),
        _ => return false,
    };
    use std::io::Read;
    let mut ba = [0u8; 64 * 1024];
    let mut bb = [0u8; 64 * 1024];
    loop {
        let na = match fa.read(&mut ba) {
            Ok(n) => n,
            Err(_) => return false,
        };
        let nb = match fb.read(&mut bb) {
            Ok(n) => n,
            Err(_) => return false,
        };
        if na != nb || ba[..na] != bb[..nb] {
            return false;
        }
        if na == 0 {
            return true;
        }
    }
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

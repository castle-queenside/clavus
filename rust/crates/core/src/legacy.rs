//! Bringing an old Clavus store into the current one.
//!
//! The Python version of Clavus kept its library in `~/Clavus/Projects`
//! and its history in `~/.clavus`. Its snapshot records carry the same
//! fields the current ones do, so the history can be brought across
//! rather than lost — but the two stores lay their files out differently,
//! and a record names the path the project had on the *old* machine.
//!
//! Nothing here is guessed. A project whose file cannot be found is
//! reported and skipped; a snapshot whose record cannot be read is
//! reported and skipped. A migration that quietly dropped half the
//! history would be worse than one that said what it could not do.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::ClavusError;
use crate::project::ProjectId;
use crate::types::{Cue, Snapshot};

/// What a migration did, and what it could not do.
#[derive(Debug, Default, Clone)]
pub struct MigrationReport {
    /// Projects whose history was brought across.
    pub imported: Vec<ImportedProject>,
    /// Projects that were left behind, and why.
    pub skipped: Vec<SkippedProject>,
    /// Snapshot records that could not be read.
    pub unreadable_snapshots: Vec<String>,
    /// Cue files that could not be read.
    pub unreadable_cues: Vec<String>,
    /// Blobs copied into the new store.
    pub blobs_copied: usize,
}

impl MigrationReport {
    /// A one-line summary for the command's output.
    pub fn summary(&self) -> String {
        format!(
            "{} project(s) imported, {} skipped, {} snapshot record(s) unreadable, \
             {} cue(s) unreadable, {} blob(s) copied",
            self.imported.len(),
            self.skipped.len(),
            self.unreadable_snapshots.len(),
            self.unreadable_cues.len(),
            self.blobs_copied,
        )
    }
}

/// A project whose history came across.
#[derive(Debug, Clone)]
pub struct ImportedProject {
    /// The name the old store used.
    pub name: String,
    /// The project's file, where it lives now.
    pub project_path: PathBuf,
    /// The number of snapshots in its chain.
    pub snapshots: usize,
    /// The number of notes that came across.
    pub cues: usize,
}

/// A project that was not brought across.
#[derive(Debug, Clone)]
pub struct SkippedProject {
    /// The name the old store used.
    pub name: String,
    /// Why it was left behind, in words the user can act on.
    pub reason: String,
}

/// Where the old store is, and where the sets are now.
///
/// The old store's `index.json` names each project's file by a path on
/// the machine it was written on — `/Users/slowhands/...`. That path
/// means nothing here, so the project is looked for by name instead.
pub struct LegacyMigration<'a> {
    /// The old `~/.clavus` directory.
    pub old_store: &'a Path,
    /// The new `~/.clavus` directory to write into.
    pub new_store: &'a Path,
    /// Where the sets are now, if they are not where the old store
    /// said.
    pub projects_root: Option<&'a Path>,
    /// The home directory of this machine, used to work out a
    /// project's identity.
    pub home: Option<&'a Path>,
    /// Say what would happen without writing anything.
    pub dry_run: bool,
}

impl<'a> LegacyMigration<'a> {
    /// Bring the old store across.
    pub fn run(&self) -> Result<MigrationReport, ClavusError> {
        let mut report = MigrationReport::default();

        let index = self.read_index()?;
        let old_objects = self.old_store.join("objects");

        for (name, entry) in index {
            // The old store kept bookkeeping keys of its own.
            if name.starts_with('_') {
                continue;
            }
            let head = entry
                .get("head")
                .and_then(|h| h.as_str())
                .map(|h| h.to_string());
            let old_path = entry
                .get("root_als")
                .and_then(|p| p.as_str())
                .map(PathBuf::from);

            let project_path = match self.find_project(&name, old_path.as_deref()) {
                Some(p) => p,
                None => {
                    report.skipped.push(SkippedProject {
                        name: name.clone(),
                        reason: format!(
                            "no project file for \"{}\" was found — its file is not where the \
                             old store said, and no file of that name is in the projects \
                             folder",
                            name
                        ),
                    });
                    continue;
                }
            };

            let project = ProjectId::new_in(&project_path, self.home);

            match self.import_project(&project, &name, head.as_deref(), &old_objects, &mut report) {
                Ok((snapshots, cues)) => report.imported.push(ImportedProject {
                    name: name.clone(),
                    project_path,
                    snapshots,
                    cues,
                }),
                Err(e) => report.skipped.push(SkippedProject {
                    name: name.clone(),
                    reason: e.to_string(),
                }),
            }
        }

        Ok(report)
    }

    /// The old store's project index.
    ///
    /// The live `index.json` is read first. The backups the old store
    /// kept are tried in turn, because a store that was interrupted
    /// mid-write may have a damaged live index and a good backup.
    fn read_index(&self) -> Result<BTreeMap<String, Value>, ClavusError> {
        let mut candidates = vec![
            self.old_store.join("index.json"),
            self.old_store.join("index.json.bak"),
            self.old_store.join("index.json.bak2"),
            self.old_store.join("index.json.bak3"),
        ];
        let mut damaged: Vec<String> = Vec::new();
        for path in candidates.drain(..) {
            if !path.exists() {
                continue;
            }
            match fs::read_to_string(&path) {
                Ok(text) => match serde_json::from_str::<BTreeMap<String, Value>>(&text) {
                    Ok(map) => return Ok(map),
                    Err(e) => damaged.push(format!("{}: {}", path.display(), e)),
                },
                Err(e) => damaged.push(format!("{}: {}", path.display(), e)),
            }
        }
        if damaged.is_empty() {
            return Err(ClavusError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!(
                    "the old store at {} has no project index",
                    self.old_store.display()
                ),
            )));
        }
        Err(ClavusError::IndexCorrupted(damaged.join("; ")))
    }

    /// Find the project's file.
    ///
    /// The old store named each project by a path on the machine it
    /// was written on — `/Users/slowhands/Clavus/Projects/Gigalo/Gigalo
    /// Project/Gigalo.als`, or the same thing with backslashes on
    /// Windows. The part after `Clavus/Projects` is still true of the
    /// library, so it is mapped onto where the sets live now. That is
    /// far better than matching on the name: a library can hold two
    /// files called `Me And You.als`, one of them a copy.
    ///
    /// When the mapping does not lead anywhere, the projects folder is
    /// searched by name.
    fn find_project(&self, name: &str, old_path: Option<&Path>) -> Option<PathBuf> {
        if let Some(p) = old_path {
            if p.exists() {
                return Some(p.to_path_buf());
            }
        }
        let root = self.projects_root?;

        // The folder of the library, as it was on the old machine.
        if let Some(relative) = relative_after_library(old_path) {
            let candidate = root.join(&relative);
            if candidate.exists() {
                return Some(candidate);
            }
        }

        // The file the old store named, and the obvious names.
        let mut candidates: Vec<String> = Vec::new();
        if let Some(p) = old_path {
            if let Some(f) = p.file_name().and_then(|n| n.to_str()) {
                candidates.push(f.to_string());
            }
        }
        if !name.ends_with(".als") && !name.ends_with(".bwproject") {
            candidates.push(format!("{}.als", name));
            candidates.push(format!("{}.bwproject", name));
        } else {
            candidates.push(name.to_string());
        }

        // A folder of the project's own, e.g. `Gigalo/Gigalo Project/`.
        let folder = root.join(name);
        if folder.is_dir() {
            if let Some(found) = find_project_file(&folder, &candidates) {
                return Some(found);
            }
        }
        find_project_file(root, &candidates)
    }

    /// Bring one project's history across.
    fn import_project(
        &self,
        project: &ProjectId,
        name: &str,
        head: Option<&str>,
        old_objects: &Path,
        report: &mut MigrationReport,
    ) -> Result<(usize, usize), ClavusError> {
        // `open`, not `new`: the store's directories are created by the
        // writes themselves, so a dry run leaves nothing behind.
        let store = crate::blob::BlobStore::open(self.new_store.join("blobs"));
        let snapshots = self.read_chain(head, old_objects, report)?;
        if snapshots.is_empty() {
            return Ok((0, 0));
        }

        let mut count = 0usize;
        for snapshot in &snapshots {
            let mut moved = snapshot.clone();
            // The record named a path on the old machine. The project
            // lives here now, and that is where a restore must write.
            moved.project_path = project.path.to_string_lossy().to_string();
            moved.project_folder = project.folder_path().to_string();

            if !self.dry_run {
                store.write_meta(&project.key, &moved)?;
            }
            count += 1;

            // The bytes the record points at.
            for hash in referenced_hashes(snapshot) {
                let name = format!("{}{}", hash, ".sample");
                let source = old_objects.join(&hash[..2]).join(&name);
                let plain = old_objects.join(&hash[..2]).join(&hash);
                for candidate in [source, plain] {
                    if candidate.exists() {
                        let dest = store
                            .objects_dir()
                            .join(&hash[..2])
                            .join(candidate.file_name().unwrap());
                        if !dest.exists() {
                            if !self.dry_run {
                                copy_blob(&candidate, &dest)?;
                            }
                            report.blobs_copied += 1;
                        }
                        break;
                    }
                }
            }

            // The project's own bytes: the old store kept the set's
            // file beside its metadata, under the same hash.
            let source = old_objects.join(&snapshot.hash[..2]).join(&snapshot.hash);
            if source.exists() {
                let dest = store
                    .objects_dir()
                    .join(&snapshot.hash[..2])
                    .join(&snapshot.hash);
                if !dest.exists() {
                    if !self.dry_run {
                        copy_blob(&source, &dest)?;
                    }
                    report.blobs_copied += 1;
                }
            }
        }

        // The project's own HEAD.
        let head_hash = head.unwrap_or_default();
        if !head_hash.is_empty() && !self.dry_run {
            store.update_ref(&project.head_ref(), head_hash)?;
        }

        let cues = self.import_cues(project, name, report)?;
        Ok((count, cues))
    }

    /// The snapshots of a project, oldest first.
    ///
    /// The chain is walked from the head through the parent links. A
    /// link that points at a record that is not there ends the walk:
    /// the rest of the chain is unreachable, and pretending otherwise
    /// would show a history with a hole in it.
    fn read_chain(
        &self,
        head: Option<&str>,
        old_objects: &Path,
        report: &mut MigrationReport,
    ) -> Result<Vec<Snapshot>, ClavusError> {
        let mut chain: Vec<Snapshot> = Vec::new();
        let mut current = head.map(|h| h.to_string());
        let mut seen: Vec<String> = Vec::new();

        while let Some(hash) = current {
            if !is_hash(&hash) {
                break;
            }
            // A cycle would never end.
            if seen.contains(&hash) {
                break;
            }
            seen.push(hash.clone());

            let path = old_objects.join(&hash[..2]).join(format!("{}.meta", hash));
            let text = match fs::read_to_string(&path) {
                Ok(t) => t,
                Err(_) => {
                    report
                        .unreadable_snapshots
                        .push(path.to_string_lossy().to_string());
                    break;
                }
            };
            let snapshot: Snapshot = match serde_json::from_str(&text) {
                Ok(s) => s,
                Err(e) => {
                    report
                        .unreadable_snapshots
                        .push(format!("{}: {}", path.display(), e));
                    break;
                }
            };
            let parent = snapshot.parent.clone();
            chain.push(snapshot);
            current = parent;
        }
        chain.reverse();
        Ok(chain)
    }

    /// Bring a project's notes across.
    fn import_cues(
        &self,
        project: &ProjectId,
        name: &str,
        report: &mut MigrationReport,
    ) -> Result<usize, ClavusError> {
        let old_dir = self.old_store.join("cues").join(name);
        if !old_dir.is_dir() {
            return Ok(0);
        }
        let store = crate::cue::CueStore::open(self.new_store, project);
        let mut count = 0usize;
        let entries = fs::read_dir(&old_dir)?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let text = match fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) => {
                    report
                        .unreadable_cues
                        .push(format!("{}: {}", path.display(), e));
                    continue;
                }
            };
            let mut cue: Cue = match serde_json::from_str(&text) {
                Ok(c) => c,
                Err(e) => {
                    report
                        .unreadable_cues
                        .push(format!("{}: {}", path.display(), e));
                    continue;
                }
            };
            // The note belongs to this project here, whatever the old
            // store did or did not say.
            cue.project = project.relay_key();
            if !self.dry_run {
                store.save(&cue)?;
            }
            count += 1;
        }
        Ok(count)
    }
}

/// Copy a blob into the new store.
///
/// The file is created here rather than copied onto the destination,
/// because a copy keeps the source's permissions and its access
/// control list. A backup's files are often read-only, and the list
/// can forbid deleting them — and a blob that cannot be replaced or
/// removed would break the store the next time those bytes were
/// written.
fn copy_blob(source: &Path, dest: &Path) -> Result<(), ClavusError> {
    use std::io::{Read, Write};

    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut input = fs::File::open(source)?;
    let mut output = fs::File::create(dest)?;
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        output.write_all(&buffer[..read])?;
    }
    output.flush()?;
    Ok(())
}

/// The hashes a snapshot record points at.
fn referenced_hashes(snapshot: &Snapshot) -> Vec<String> {
    let mut hashes = Vec::new();
    for hash in &snapshot.sample_hashes {
        if is_hash(hash) {
            hashes.push(hash.clone());
        }
    }
    hashes
}

/// The part of the old path that follows the library folder.
///
/// The old store's paths look like
/// `.../Clavus/Projects/Gigalo/Gigalo Project/Gigalo.als` — the part
/// after `Projects` is still true of the library wherever it is now.
fn relative_after_library(old_path: Option<&Path>) -> Option<PathBuf> {
    let text = old_path?.to_string_lossy().replace('\\', "/");
    let parts: Vec<&str> = text.split('/').filter(|p| !p.is_empty()).collect();
    // The library folder is the one named `Projects`. `Clavus` is
    // accepted as a fallback for a path that does not have it, but it
    // is looked for second — the Clavus folder holds the library, so
    // stopping there would leave `Projects` in the path.
    let at = parts
        .iter()
        .position(|p| p.eq_ignore_ascii_case("projects"))
        .or_else(|| parts.iter().position(|p| p.eq_ignore_ascii_case("clavus")))?;
    let after = &parts[at + 1..];
    if after.is_empty() {
        return None;
    }
    Some(after.iter().collect())
}

/// The first project file of the given names below `root`.
fn find_project_file(root: &Path, names: &[String]) -> Option<PathBuf> {
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > 12 {
            continue;
        }
        let entries = match fs::read_dir(&dir) {
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
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name.starts_with('.') {
                    continue;
                }
                stack.push((path, depth + 1));
                continue;
            }
            let found = path.file_name().and_then(|n| n.to_str());
            if let Some(found) = found {
                if names.iter().any(|n| n == found) {
                    return Some(path);
                }
            }
        }
    }
    None
}

/// A SHA-256 hash in lowercase hex.
fn is_hash(s: &str) -> bool {
    s.len() == 64
        && s.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

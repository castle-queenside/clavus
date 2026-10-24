//! Sample audit and collection.
//!
//! This module replaces the Python `except: pass` silent failure with
//! **fail-fast** sample integrity checking.
//!
//! ## The Problem (Python, before)
//!
//! ```python
//! try:
//!     ...resolve sample paths...
//! except: pass  # SILENT FAILURE — sample silently missing on restore
//! ```
//!
//! ## The Fix (Rust, now)
//!
//! `SampleAuditReport.ensure_clean()` must be called before any snapshot is saved.
//! If samples are missing, the snapshot is BLOCKED with a clear error message.
//!
//! ## Sample Resolution
//!
//! Paths in `.als` files can be:
//! - Absolute (e.g. `/Users/slowhands/Audio/kick.wav`)
//! - Relative to project root (e.g. `Samples/kick.wav`)
//! - Filename-only (Live 10 bug where the absolute path is lost)
//!
//! Resolution order: relative → absolute → filename glob
//!
//! On macOS, filename matching is case-insensitive (APFS).

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use tracing::{debug, error, info, warn};
use crate::blob::BlobStore;
use crate::error::ClavusError;
use crate::types::{SampleAuditReport, SampleRef};

// ─── Sample reference extraction ─────────────────────────────────────────────

/// A resolved sample with its original path and resolved location.
#[derive(Debug, Clone)]
pub struct ResolvedSample {
    /// Original path as stored in the DAW file.
    pub original: PathBuf,
    /// Resolved absolute path on disk (if found).
    pub resolved: Option<PathBuf>,
    /// Relative path from project root (for display/rewriting).
    pub relpath: Option<String>,
    /// SHA-256 of file contents.
    pub hash: Option<String>,
    /// File size in bytes.
    pub size_bytes: u64,
}

// ─── Live 10/11 sample path extraction ───────────────────────────────────────

/// Extract audio sample paths from a raw `.als` XML string (already decompressed).
/// Handles the `<RelativePathElement Dir="..."/>` + `<Name Value="..."/>` chain format.
/// Extract audio sample paths from a raw `.als` XML string (already decompressed).
///
/// The extraction itself lives in [`crate::als_refs`] so that the audit and
/// the Ableton parser can never drift apart.
pub fn extract_als_sample_paths_from_xml(xml: &str) -> Vec<PathBuf> {
    crate::als_refs::extract_sample_refs_from_xml(xml)
}

// ─── Audit ───────────────────────────────────────────────────────────────────

/// Decompress an `.als` file's bytes.
///
/// Live 9 and newer write gzip-compressed XML. Live 8 (and files that have
/// been through a hand-editing or recovery tool) can be plain XML, so fall
/// back to using the bytes as-is when the gzip magic number is absent.
///
/// Returns the decompressed bytes and whether the source was gzip-compressed.
pub fn decompress_als_bytes(raw: &[u8]) -> std::io::Result<(Vec<u8>, bool)> {
    const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];
    if raw.len() >= 2 && raw[0] == GZIP_MAGIC[0] && raw[1] == GZIP_MAGIC[1] {
        Ok((flate2_decode(raw)?, true))
    } else if raw.len() >= 2 && raw[0] == 0xab && raw[1] == 0x1e {
        // Live's own compressed document format, which it used before
        // gzip. It is a binary serialisation, not XML, so there is
        // nothing to decompress into text.
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "this set uses Live's old compressed format, which Clavus cannot read; \
             open it in Live and save it again to convert it",
        ))
    } else {
        // Not gzip — treat as plain XML.
        Ok((raw.to_vec(), false))
    }
}

fn flate2_decode(data: &[u8]) -> std::io::Result<Vec<u8>> {
    use flate2::read::GzDecoder;
    let mut decoder = GzDecoder::new(data);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out)?;
    Ok(out)
}

/// Run a full sample audit on a project file.
/// Returns a report with collected, missing, and outside-project samples.
pub fn audit_project(project_path: &Path) -> Result<SampleAuditReport, ClavusError> {
    audit_project_with(project_path, &crate::relocate::Relocations::default())
}

/// The same audit, with the user's relocation map in hand.
///
/// A set records the absolute path of every sample it uses. When a drive
/// is renamed or a library moves, those paths stop resolving and the set
/// looks like it has lost its audio. The map says where those paths went,
/// and the audit asks it before it calls a sample missing.
pub fn audit_project_with(
    project_path: &Path,
    relocations: &crate::relocate::Relocations,
) -> Result<SampleAuditReport, ClavusError> {
    let project_root = project_root_of(project_path)
        .ok_or_else(|| ClavusError::ParseError("no parent directory for project".into()))?;

    let ext = project_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default();

    let refs = if ext == "bwproject" {
        extract_bitwig_sample_paths(project_path)?
    } else {
        // Ableton .als: gzip-compressed XML
        let raw = fs::read(project_path)
            .map_err(|e| ClavusError::ParseError(format!("read {}: {}", project_path.display(), e)))?;
        let (xml_bytes, _gzipped) = decompress_als_bytes(&raw)
            .map_err(|e| ClavusError::ParseError(format!("gzip: {}", e)))?;
        let xml = String::from_utf8(xml_bytes)
            .map_err(ClavusError::from)?;
        // Reject anything that is not a Live set before auditing it — a
        // corrupt or unrelated file must not audit as "0 samples present".
        validate_als_xml(&xml)?;
        extract_als_sample_paths_from_xml(&xml)
    };

    // A `.bwproject` archive usually carries a copy of every sample it
    // uses. Those samples are not on disk, but they are not lost either,
    // so the audit has to know what the archive holds before it calls a
    // sample missing.
    let embedded = if ext == "bwproject" {
        let entries = crate::bitwig_archive::list_entries(project_path)?;
        entries
            .iter()
            .filter(|n| !n.ends_with('/') && !n.ends_with('\\'))
            .cloned()
            .collect::<Vec<String>>()
    } else {
        Vec::new()
    };

    debug!(
        "audit_project: {} sample refs ({} embedded in archive) from {}",
        refs.len(),
        embedded.len(),
        project_path.display()
    );
    audit_sample_refs(
        &refs,
        project_root,
        &embedded,
        ext == "bwproject",
        relocations,
    )
}

/// Validate that `xml` is an Ableton Live set.
///
/// The sample extractor is deliberately lenient — it only looks for
/// `<SampleRef>` elements — so without this check a corrupt or unrelated
/// file would audit as "0 samples present" and look clean.
///
/// The whole document is parsed, so a truncated or malformed file is
/// rejected even when its opening tags look like a Live set.
pub fn validate_als_xml(xml: &str) -> Result<(), ClavusError> {
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.trim_text(true);
    let mut buf = Vec::new();
    let mut saw_liveset = false;
    // quick-xml does not report an element that is never closed, so the
    // nesting depth is tracked here to catch a truncated document.
    let mut depth: i64 = 0;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Start(e)) => {
                depth += 1;
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if tag == "LiveSet" || tag == "Ableton" {
                    saw_liveset = true;
                }
            }
            Ok(quick_xml::events::Event::Empty(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if tag == "LiveSet" || tag == "Ableton" {
                    saw_liveset = true;
                }
            }
            Ok(quick_xml::events::Event::End(_)) => {
                depth -= 1;
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Err(e) => {
                return Err(ClavusError::ParseError(format!("malformed XML: {}", e)));
            }
            _ => {}
        }
        buf.clear();
    }

    if depth != 0 {
        return Err(ClavusError::ParseError(
            "malformed XML: the document ends with an unclosed element".into(),
        ));
    }

    // The XML reader accepts a closing tag whose `>` is missing, so a set
    // truncated by a single byte used to look complete. A well-formed
    // document always ends with `>`.
    if !crate::als_refs::is_complete_document(xml) {
        return Err(ClavusError::ParseError(
            "malformed XML: the document is truncated".into(),
        ));
    }

    if !saw_liveset {
        return Err(ClavusError::ParseError(
            "not an Ableton Live project: no <Ableton> or <LiveSet> element".into(),
        ));
    }
    Ok(())
}

/// Extract audio sample paths from a Bitwig `.bwproject` ZIP archive.
///
/// Bitwig stores sample references in `project.xml` with various tag/attribute
/// combinations. We extract all attribute values that look like absolute paths
/// or contain audio file extensions.
fn extract_bitwig_sample_paths(project_path: &Path) -> Result<Vec<PathBuf>, ClavusError> {
    use std::io::Read;

    let file = std::fs::File::open(project_path)
        .map_err(|e| ClavusError::ParseError(format!("open {}: {}", project_path.display(), e)))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| ClavusError::ParseError(format!("zip open: {}", e)))?;

    let mut xml_bytes: Vec<u8> = Vec::new();
    let mut found = false;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)
            .map_err(|e| ClavusError::ParseError(format!("zip entry: {}", e)))?;
        let name = entry.name();
        if name == "project.xml" || name.ends_with("/project.xml") || name.ends_with("\\project.xml") {
            entry.read_to_end(&mut xml_bytes)
                .map_err(|e| ClavusError::ParseError(format!("read project.xml: {}", e)))?;
            found = true;
            break;
        }
    }

    if !found || xml_bytes.is_empty() {
        return Err(ClavusError::ParseError(
            "not a Bitwig project archive: no project.xml found in the .bwproject".into(),
        ));
    }

    let xml = String::from_utf8(xml_bytes)
        .map_err(ClavusError::from)?;

    // A bundle whose project.xml is broken is a broken bundle. Auditing it
    // as "0 samples present" hid real corruption.
    validate_bitwig_xml(&xml)?;

    // Parse the XML and extract attribute values that look like audio paths
    let mut refs = Vec::new();
    let mut reader = quick_xml::Reader::from_str(&xml);
    reader.trim_text(true);
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Start(e)) | Ok(quick_xml::events::Event::Empty(e)) => {
                for attr in e.attributes().flatten() {
                    // The XML holds the path escaped: a file called
                    // `R&B/kick.wav` is written `R&amp;B/kick.wav`. The
                    // Ableton parser decodes it, and the Bitwig parser
                    // has to decode it too — otherwise a sample that is
                    // really on disk is reported missing.
                    let val = crate::als_refs::decode_xml_entities(
                        &String::from_utf8_lossy(attr.value.as_ref()),
                    );
                    if looks_like_audio_path(&val) {
                        refs.push(PathBuf::from(val));
                    }
                }
            }
            Ok(quick_xml::events::Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }

    debug!("extract_bitwig_sample_paths: {} refs from {}", refs.len(), project_path.display());
    Ok(refs)
}

/// The file extensions Clavus treats as audio.
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "wav", "aiff", "aif", "aifc", "mp3", "flac", "ogg", "oga", "m4a", "wma", "opus", "aac",
];

/// Whether `path` ends in a known audio extension.
///
/// The extension is the only reliable signal. Treating *every* absolute
/// path in a project as a sample made Bitwig preset references
/// (`/Users/me/Library/Presets/EQ.preset`) look like missing audio, which
/// blocked snapshots of perfectly healthy projects.
pub fn has_audio_extension(path: &str) -> bool {
    let lower = path.to_lowercase();
    AUDIO_EXTENSIONS
        .iter()
        .any(|ext| lower.ends_with(&format!(".{}", ext)))
}

/// Returns true if `val` looks like a path to an audio file.
///
/// A path must name an audio file: an ID or a preset path is not a sample.
fn looks_like_audio_path(val: &str) -> bool {
    if val.is_empty() || val.len() < 4 {
        return false;
    }
    // Skip UUIDs/IDs (no slashes, no dots, or very short/long)
    if !val.contains('/') && !val.contains('\\') && !val.contains('.') {
        return false;
    }
    has_audio_extension(val)
}

/// Validate that `xml` is a Bitwig project document.
///
/// A `.bwproject` archive without a readable `project.xml` is not a
/// project. Auditing it as "0 samples present" made a broken bundle look
/// perfectly healthy.
pub fn validate_bitwig_xml(xml: &str) -> Result<(), ClavusError> {
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.trim_text(true);
    let mut buf = Vec::new();
    let mut saw_project = false;
    // quick-xml does not report an element that is never closed, so the
    // nesting depth is tracked here to catch a truncated document.
    let mut depth: i64 = 0;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Start(e)) => {
                depth += 1;
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if tag == "Project" {
                    saw_project = true;
                }
            }
            Ok(quick_xml::events::Event::Empty(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if tag == "Project" {
                    saw_project = true;
                }
            }
            Ok(quick_xml::events::Event::End(_)) => {
                depth -= 1;
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Err(e) => {
                return Err(ClavusError::ParseError(format!(
                    "malformed Bitwig project.xml: {}",
                    e
                )));
            }
            _ => {}
        }
        buf.clear();
    }

    if depth != 0 {
        return Err(ClavusError::ParseError(
            "malformed Bitwig project.xml: the document ends with an unclosed element".into(),
        ));
    }

    if !saw_project {
        return Err(ClavusError::ParseError(
            "not a Bitwig project: project.xml has no <Project> element".into(),
        ));
    }
    Ok(())
}

/// The directory that contains a project file.
///
/// `Path::parent()` returns an *empty* path for a bare filename like
/// `demo.als`, and `strip_prefix("")` matches every path — which would make
/// every sample look like it lives inside the project. Use `.` instead, so
/// the root resolves to the current directory.
pub fn project_root_of(project_path: &Path) -> Option<&Path> {
    match project_path.parent() {
        Some(p) if !p.as_os_str().is_empty() => Some(p),
        _ => Some(Path::new(".")),
    }
}

/// Audit a list of raw sample paths against a project root.
///
/// Returns a [`SampleAuditReport`] with three categories:
/// - **collected**: found and inside project folder -> safe
/// - **missing**: not found on disk -> BLOCK snapshot
/// - **outside_project**: found but outside project folder -> BLOCK snapshot
fn audit_sample_refs(
    paths: &[PathBuf],
    project_root: &Path,
    archive_files: &[String],
    is_bundle: bool,
    relocations: &crate::relocate::Relocations,
) -> Result<SampleAuditReport, ClavusError> {
    let mut collected = Vec::new();
    let mut missing = Vec::new();
    let mut outside_project = Vec::new();
    let mut embedded = Vec::new();
    let mut factory = Vec::new();

    // De-duplicate references: a sample used by six clips is one file, and
    // reporting it six times hides the real problem behind noise.
    let mut unique_paths: Vec<PathBuf> = Vec::with_capacity(paths.len());
    let mut seen_paths: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    for p in paths {
        if seen_paths.insert(p.clone()) {
            unique_paths.push(p.clone());
        }
    }
    let paths = unique_paths.as_slice();

    // Canonicalize the project root so the inside/outside comparison works
    // regardless of whether the caller passed a relative project path.
    // Without this, `clavus audit ./myproj.als` would flag every sample as
    // "outside project" because the root is relative but resolved sample
    // paths are absolute.
    let abs_root = fs::canonicalize(project_root)
        .or_else(|_| {
            // A relative root like "." needs a base to resolve against.
            if project_root.as_os_str().is_empty() {
                std::env::current_dir()
            } else {
                Err(std::io::Error::other("no such directory"))
            }
        })
        .unwrap_or_else(|_| project_root.to_path_buf());

    // An empty root would make `strip_prefix` match every path, so refuse to
    // treat anything as "inside the project" in that case.
    let root_is_usable = !abs_root.as_os_str().is_empty();

    // Where a relative reference may live, besides the project folder.
    //
    // A Live set can point its samples at the machine's *user folder*
    // rather than at itself. The path then starts with empty elements,
    // which stand for the user folder's root. The user folder is a
    // setting of the machine that made the set, not something the set
    // records, so the places it is likely to be are tried in turn.
    let extra_roots = user_folder_roots(project_root);

    for path in paths {
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        // Try multiple resolution strategies:
        // 1. Relative to the project root (most common). An absolute
        //    reference names a place on disk directly — `join` on an
        //    absolute path replaces the base — so one candidate covers
        //    both.
        // 2. Relative to project root, matching each path component
        //    case-insensitively — Live has been known to change the case
        //    of a folder in a saved set.
        //
        // A reference is resolved against the *project*, never against
        // the process's current directory. The working directory is not
        // part of the set: a file with the same name in whatever folder
        // the command happened to be run from could be picked up instead
        // of the copy inside the project, and the report then claimed the
        // sample was outside the project. That made the audit depend on
        // where it was run, and made two runs of the same command
        // disagree.
        //
        // A search for *any* file with a matching name under the project
        // used to be the third strategy. It accepted a file the set never
        // referenced, so a set pointing at a sample that is not there
        // audited as clean and the snapshot recorded a path that does not
        // exist.
        let mut candidates: Vec<PathBuf> = vec![abs_root.join(path)];

        // A bundle is a self-contained project: a relative reference in
        // it points *inside the bundle*. That is where the sample lives,
        // so the bundle is asked before the disk. A file with the same
        // name sitting next to the bundle is not the sample the project
        // uses, and treating it as one hides a bundle that would open
        // without its audio.
        if is_bundle && path.is_relative() {
            if let Some(entry) = archive_entry_at(archive_files, path) {
                embedded.push(SampleRef {
                    path: path.clone(),
                    filename,
                    resolved_path: None,
                    relpath_from_project: None,
                    hash: None,
                    size_bytes: 0,
                    archive_entry: Some(entry),
                });
                continue;
            }
        }

        if path.is_relative() {
            if let Some(found) = resolve_case_insensitive(&abs_root, path) {
                if !candidates.contains(&found) {
                    candidates.push(found);
                }
            }
            // The same reference, looked for in the places a user folder
            // is usually kept. A sample found here is *outside* the
            // project, and the audit says so — it is collectable, not lost.
            for root in &extra_roots {
                let candidate = root.join(path);
                if !candidates.contains(&candidate) {
                    candidates.push(candidate);
                }
            }
        }

        // Where the user says this path moved to.
        //
        // This comes last on purpose: a path that still resolves where
        // the set says it does is used as it is, and the map is only
        // asked about the ones that no longer resolve. A map that is out
        // of date cannot quietly move a sample that was there all along.
        for candidate in relocations.candidates(path) {
            if !candidates.contains(&candidate) {
                candidates.push(candidate);
            }
        }

        let resolved = candidates.into_iter().find(|c| c.is_file());

        // Canonicalize the resolved path so it can be compared against the
        // canonical root (both may differ in symlink form or absolute/relative).
        let resolved_path = resolved.map(|p| {
            fs::canonicalize(&p).unwrap_or(p)
        });
        let relpath = resolved_path.as_ref().and_then(|p| {
            if !root_is_usable {
                return None;
            }
            p.strip_prefix(&abs_root)
                .ok()
                .map(|s| s.to_string_lossy().replace('\\', "/"))
        });

        let hash = resolved_path.as_ref().and_then(|p| {
            fs::read(p).ok().map(|data| BlobStore::sha256(&data))
        });
        let size = resolved_path
            .as_ref()
            .and_then(|p| p.metadata().ok())
            .map(|m| m.len())
            .unwrap_or(0);

        let ref_ = SampleRef {
            path: path.clone(),
            filename,
            resolved_path,
            relpath_from_project: relpath.clone(),
            hash: hash.clone(),
            size_bytes: size,
            archive_entry: None,
        };

        // A sample that is not on disk may still be inside the project
        // archive. That is a present sample, not a missing one.
        //
        // Two cases are possible. The reference may already point at the
        // sample's place inside the bundle — then the bundle is complete
        // and nothing has to be done. Or the reference points somewhere
        // else (an old absolute path, say) while the bundle carries a
        // copy under the same file name — then the bundle can still be
        // repaired by pointing the reference at its own copy.
        let archive_entry = if ref_.resolved_path.is_none() {
            archive_files
                .iter()
                .find(|e: &&String| {
                    e.rsplit(['/', '\\'])
                        .next()
                        .unwrap_or(e)
                        .eq_ignore_ascii_case(&ref_.filename)
                })
                .cloned()
        } else {
            None
        };
        let in_archive = archive_entry.is_some();

        let ref_ = SampleRef {
            archive_entry,
            ..ref_
        };

        match (ref_.resolved_path.is_some(), relpath.is_some()) {
            (true, true) => {
                // Found AND inside project folder
                collected.push(ref_);
            }
            (true, false) => {
                // Found but OUTSIDE project folder
                outside_project.push(ref_);
            }
            (false, _) if in_archive => {
                // Inside the project archive — present, just not extracted
                embedded.push(ref_);
            }
            (false, _) if is_factory_library_path(path) => {
                // Live's own factory library. Not the user's audio, and
                // it comes back whenever Live is installed.
                factory.push(ref_);
            }
            (false, _) => {
                // NOT FOUND
                missing.push(ref_);
            }
        }
    }

    let report = SampleAuditReport {
        collected,
        missing,
        outside_project,
        embedded,
        factory,
    };

    if !report.is_clean() {
        if !report.missing.is_empty() {
            warn!(
                "⚠ {} sample(s) missing — run `clavus collect` first",
                report.missing.len()
            );
        }
        if !report.outside_project.is_empty() {
            warn!(
                "⚠ {} sample(s) outside project folder — run `clavus collect` to copy them in",
                report.outside_project.len()
            );
        }
    } else {
        info!("✓ Sample audit passed: {} samples, all inside project folder", report.total());
    }

    Ok(report)
}

// ─── Collection ───────────────────────────────────────────────────────────────

/// Whether a name is free for a source.
///
/// A name is taken when another source in this run claimed it, or when
/// a file with that name is already in the project. A file that is
/// already there with the very same bytes is not a clash: it is the
/// sample the project already carries, and copying over it would be a
/// no-op.
fn is_free(candidate: &str, samples_dir: &Path, source: &Path) -> bool {
    let dest = samples_dir.join(candidate);
    if !dest.exists() {
        return true;
    }
    // The same file collected twice is the same file.
    if dest == source {
        return true;
    }
    // The same bytes: the project already carries this sample, so the
    // name is free — there is nothing to copy and nothing to lose.
    // Different bytes: the name is taken, and the sample gets a name
    // of its own. The user's file is never written over.
    same_bytes(&dest, source)
}

/// Whether two files hold exactly the same bytes.
///
/// The files are compared in chunks. Reading them whole would cost the
/// size of every sample in memory, and a project with a few large
/// stems would take gigabytes to answer a question a stream answers
/// in a moment.
fn same_bytes(a: &Path, b: &Path) -> bool {
    use std::io::Read;
    let (Ok(mut fa), Ok(mut fb)) = (fs::File::open(a), fs::File::open(b)) else {
        return false;
    };
    let (Ok(sa), Ok(sb)) = (fa.metadata(), fb.metadata()) else {
        return false;
    };
    if sa.len() != sb.len() {
        return false;
    }
    let mut ba = [0u8; 64 * 1024];
    let mut bb = [0u8; 64 * 1024];
    loop {
        let ra = match fa.read(&mut ba) {
            Ok(n) => n,
            Err(_) => return false,
        };
        let rb = match fb.read(&mut bb) {
            Ok(n) => n,
            Err(_) => return false,
        };
        if ra != rb {
            return false;
        }
        if ra == 0 {
            return true;
        }
        if ba[..ra] != bb[..rb] {
            return false;
        }
    }
}

/// Claim a unique destination file name for a sample.
///
/// Two samples can share a file name — `a/loop.wav` and `b/loop.wav` are
/// different audio. Copying both to `Samples/loop.wav` merged them and one
/// track played the other's audio, so a clash is given a name of its own:
/// `a_loop.wav`, then `b_loop.wav`, and finally a name carrying a short
/// digest of the source path.
///
/// A file that is already in the project is a clash too, unless it holds
/// the very same audio. The user's own sample is never written over.
fn unique_destination(
    claimed: &mut HashMap<String, PathBuf>,
    samples_dir: &Path,
    filename: &str,
    source: &Path,
) -> String {
    unique_destination_in(claimed, samples_dir, filename, source, &[])
}

/// The same, but for a bundle: the entries the archive already carries
/// are names that are taken.
fn unique_destination_in(
    claimed: &mut HashMap<String, PathBuf>,
    samples_dir: &Path,
    filename: &str,
    source: &Path,
    archive_entries: &[String],
) -> String {
    // The same source claimed twice keeps the same name.
    if let Some(existing) = claimed.get(filename) {
        if existing == source {
            return filename.to_string();
        }
    }

    let mut candidate = filename.to_string();
    if !claimed.contains_key(&candidate)
        && is_free(&candidate, samples_dir, source)
        && !archive_takes(archive_entries, &candidate)
    {
        claimed.insert(candidate.clone(), source.to_path_buf());
        return candidate;
    }

    // Disambiguate with the source's parent folder name.
    let parent = source
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let stem = Path::new(filename)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| filename.to_string());
    let ext = Path::new(filename)
        .extension()
        .map(|s| format!(".{}", s.to_string_lossy()))
        .unwrap_or_default();

    if !parent.is_empty() {
        candidate = format!("{}_{}{}", parent, stem, ext);
        if !claimed.contains_key(&candidate)
            && is_free(&candidate, samples_dir, source)
            && !archive_takes(archive_entries, &candidate)
        {
            claimed.insert(candidate.clone(), source.to_path_buf());
            return candidate;
        }
    }

    // Last resort: a short digest of the full source path.
    let digest = BlobStore::sha256(source.to_string_lossy().as_bytes());
    candidate = format!("{}_{}{}", stem, &digest[..8], ext);
    claimed.insert(candidate.clone(), source.to_path_buf());
    candidate
}

/// Does the bundle already carry a file with this name?
fn archive_takes(archive_entries: &[String], filename: &str) -> bool {
    archive_entries
        .iter()
        .any(|entry| entry.rsplit('/').next() == Some(filename))
}

/// Collect missing/outside-project samples into the project folder.
///
/// Copies all samples from `outside_project` and `missing` lists into
/// `project_root/Samples/`, preserving directory structure.
/// Rewrites the project file to point to the new local paths.
///
/// Returns a [`CollectReport`] with what was copied and what failed.
pub fn collect_samples(
    project_path: &Path,
    report: &SampleAuditReport,
) -> Result<CollectReport, ClavusError> {
    let project_root = project_root_of(project_path)
        .ok_or_else(|| ClavusError::ParseError("no parent directory".into()))?;

    let samples_dir = project_root.join("Samples");
    // The directory is only created once there is something to put in it,
    // so a failed collect does not leave an empty `Samples/` behind.

    let mut copied = Vec::new();
    let mut failed = Vec::new();
    // The samples that were already inside the project bundle, and the
    // ones that were added to it.
    let mut already_inside = 0usize;
    let mut added_to_bundle = 0usize;
    // The references that were pointed at the copy inside the bundle.
    // The bundle already carried the sample; the project did not point
    // at it. The report has to say which of the two happened.
    let mut pointed_inside = 0usize;
    // Map: original path as stored in XML → new relative path (Unix-slash)
    let mut path_replacements: HashMap<String, String> = HashMap::new();
    // The references that were pointed at a file the project already
    // carried, rather than at a copy that had to be made.
    let mut repointed_in_project = 0usize;

    // A `.bwproject` archive carries its own copies of the samples it
    // uses. Those can be extracted even though nothing is on disk.
    let is_bitwig = project_path
        .extension()
        .and_then(|s| s.to_str())
        .map(|e| e.eq_ignore_ascii_case("bwproject"))
        .unwrap_or(false);
    let archive_entries = if is_bitwig {
        crate::bitwig_archive::list_entries(project_path).unwrap_or_default()
    } else {
        Vec::new()
    };

    // Collect outside_project + missing + embedded (samples we CAN find)
    let candidates = report
        .outside_project
        .iter()
        .chain(report.missing.iter())
        .chain(report.embedded.iter());

    // Two different samples can share a file name. Copying both to
    // `Samples/loop.wav` merged them, and the set then played one track's
    // audio for another. Every destination is therefore claimed by exactly
    // one source, and a clash gets a distinct name.
    let mut claimed: HashMap<String, PathBuf> = HashMap::new();

    for ref_ in candidates {
        if let Some(ref resolved) = ref_.resolved_path {
            let orig = ref_.path.to_string_lossy().replace('\\', "/");
            if is_bitwig {
                // A bundle is a self-contained project: the sample's
                // bytes go *into* the archive and the reference points
                // at the copy inside it. A copy written next to the
                // bundle would leave the bundle pointing at something
                // it does not carry, and the project would open
                // without its audio.
                let filename = unique_destination_in(
                    &mut claimed,
                    &samples_dir,
                    &ref_.filename,
                    resolved,
                    &archive_entries,
                );
                let internal = format!("samples/{}", filename);
                let bytes = match fs::read(resolved) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        error!("Failed to read {}: {}", resolved.display(), e);
                        failed.push((ref_.path.clone(), e.to_string()));
                        continue;
                    }
                };
                match crate::bitwig_archive::add_entry(project_path, &internal, &bytes) {
                    Ok(()) => {
                        match crate::bitwig_archive::rewrite_sample_path(
                            project_path,
                            &orig,
                            &internal,
                        ) {
                            Ok(true) => {
                                info!("Collected {} into the project bundle", filename);
                                copied.push(project_path.to_path_buf());
                                added_to_bundle += 1;
                            }
                            Ok(false) => {
                                error!(
                                    "The reference to {} was not found in the project description",
                                    ref_.filename
                                );
                                failed.push((
                                    ref_.path.clone(),
                                    "the reference was not found in the project description"
                                        .to_string(),
                                ));
                            }
                            Err(e) => {
                                error!("Failed to repair the reference to {}: {}", ref_.filename, e);
                                failed.push((ref_.path.clone(), e.to_string()));
                            }
                        }
                    }
                    Err(e) => {
                        error!("Failed to collect {} into the bundle: {}", ref_.filename, e);
                        failed.push((ref_.path.clone(), e.to_string()));
                    }
                }
                continue;
            }
            let filename =
                unique_destination(&mut claimed, &samples_dir, &ref_.filename, resolved);
            let dest = samples_dir.join(&filename);
            if let Err(e) = fs::create_dir_all(&samples_dir) {
                error!("Failed to create {}: {}", samples_dir.display(), e);
                failed.push((ref_.path.clone(), e.to_string()));
                continue;
            }
            // A file that is already there with the same bytes is the
            // sample the project already carries. It is not copied
            // over — the user's file stays exactly as it is — and the
            // reference simply points at it.
            let already_there = same_bytes(&dest, resolved);
            let copied_result = if already_there {
                info!("{} is already in the project", filename);
                Ok(0)
            } else {
                fs::copy(resolved, &dest)
            };
            match copied_result {
                Ok(bytes) => {
                    info!("Collected: {} ({} bytes)", filename, bytes);
                    copied.push(dest.clone());
                    // Build replacement: original path in XML → new relative path
                    let new_rel = format!("Samples/{}", filename);
                    path_replacements.insert(orig, new_rel);
                }
                Err(e) => {
                    error!("Failed to copy {}: {}", ref_.filename, e);
                    failed.push((ref_.path.clone(), e.to_string()));
                }
            }
        } else if let Some(entry) = ref_.archive_entry.clone().or_else(|| {
            crate::bitwig_archive::find_entry_by_filename(&archive_entries, &ref_.filename)
        }) {
            // The sample is inside the project bundle.
            //
            // A bundle is a self-contained project folder: a reference
            // inside it is resolved *within* the bundle. So the repair is
            // to point the reference at the sample's place inside the
            // bundle — the file is already there — and not to write a
            // copy next to the project, which would leave the bundle
            // pointing at something it does not carry.
            let internal = entry.replace('\\', "/");
            let orig = ref_.path.to_string_lossy().replace('\\', "/");
            if orig == internal {
                // The bundle already points at its own copy.
                info!("{} is already inside the project bundle", ref_.filename);
                copied.push(project_path.to_path_buf());
                already_inside += 1;
            } else {
                match crate::bitwig_archive::rewrite_sample_path(project_path, &orig, &internal) {
                    Ok(true) => {
                        info!(
                            "Pointed {} at the copy inside the project bundle ({})",
                            ref_.filename, internal
                        );
                        copied.push(project_path.to_path_buf());
                        already_inside += 1;
                        pointed_inside += 1;
                    }
                    Ok(false) => {
                        // The reference was not found in the project
                        // description, so nothing was changed. Say so
                        // rather than claim the project was repaired.
                        error!(
                            "The reference to {} was not found in the project description",
                            ref_.filename
                        );
                        failed.push((
                            ref_.path.clone(),
                            "the reference was not found in the project description".to_string(),
                        ));
                    }
                    Err(e) => {
                        error!("Failed to repair the reference to {}: {}", ref_.filename, e);
                        failed.push((ref_.path.clone(), e.to_string()));
                    }
                }
            }
        } else {
            // The file is not where the project says it is.
            //
            // It may still be *in* the project. A set that another tool
            // collected keeps the old references — pointing at the
            // machine the set came from — while the audio sits in the
            // project's own sample folder. Re-pointing the reference
            // at the file that is already there is the repair, and it
            // is only safe when the name matches exactly one file.
            let orig = ref_.path.to_string_lossy().replace('\\', "/");
            match find_sample_in_project(project_root, &ref_.path, &ref_.filename) {
                Ok(Some(local)) => {
                    let rel = relative_to(&local, project_root);
                    info!(
                        "Pointed {} at the copy already in the project ({})",
                        ref_.filename, rel
                    );
                    path_replacements.insert(orig, rel);
                    repointed_in_project += 1;
                }
                Ok(None) => {
                    failed.push((ref_.path.clone(), "file not found on disk".to_string()));
                }
                Err(e) => {
                    failed.push((ref_.path.clone(), e.to_string()));
                }
            }
        }
    }

    // Rewrite project file sample paths
    let mut rewritten = 0usize;
    if !path_replacements.is_empty() {
        if is_bitwig {
            // The reference lives inside the archive's project.xml.
            for (old, new) in &path_replacements {
                if crate::bitwig_archive::rewrite_sample_path(project_path, old, new)? {
                    rewritten += 1;
                } else {
                    error!(
                        "The reference to {} was not found in the project description",
                        old
                    );
                    failed.push((
                        PathBuf::from(old),
                        "the reference was not found in the project description".to_string(),
                    ));
                }
            }
        } else {
            // A reference that cannot be rewritten is a failure, not a
            // silent success: the sample is in the project now, but the
            // project still points at the outside file.
            let wanted = path_replacements.len();
            let unmatched = rewrite_project_sample_paths(project_path, path_replacements)?;
            rewritten = wanted - unmatched.len();
            for old in unmatched {
                error!("The reference to {} was not found in the project file", old);
                failed.push((
                    PathBuf::from(&old),
                    "the reference could not be rewritten — the project still points at the outside file"
                        .to_string(),
                ));
            }
        }
    }

    Ok(CollectReport {
        copied,
        failed,
        already_inside,
        added_to_bundle,
        pointed_inside,
        rewritten,
        repointed_in_project,
    })
}

/// Report of a collect operation.
#[derive(Debug)]
pub struct CollectReport {
    /// Paths that were successfully copied into the project.
    pub copied: Vec<PathBuf>,
    /// (path, error) pairs for samples that could not be copied.
    pub failed: Vec<(PathBuf, String)>,
    /// The samples that were already inside the project bundle.
    ///
    /// Nothing was copied for them: the bundle already carried them.
    pub already_inside: usize,
    /// The samples that were added to the project bundle.
    pub added_to_bundle: usize,
    /// The references that were pointed at the copy the bundle already
    /// carried.
    ///
    /// The sample was inside the bundle; the project description still
    /// pointed somewhere else. The bundle was repaired, and the report
    /// has to say so rather than claim that nothing needed doing.
    pub pointed_inside: usize,
    /// The references that were rewritten to point at the collected
    /// samples.
    ///
    /// Fewer than the number of collected samples means the project
    /// still points at some of the outside files.
    pub rewritten: usize,
    /// The references that were pointed at a file the project already
    /// carried.
    ///
    /// The set pointed at a path on another machine while the audio was
    /// already in the project. Nothing was copied; the reference was
    /// pointed at the file that was there.
    pub repointed_in_project: usize,
}

/// Find the file a missing reference is really pointing at.
///
/// A set made on another machine points at a path that does not exist
/// here. The audio may still be in the project — a set that another
/// tool collected keeps the old references while the audio was
/// brought along.
///
/// Two rules, in order:
///
/// 1. **The same path.** The reference is an absolute path on another
///    machine, but it ends in the project's own layout. The longest
///    tail of it that exists in the project is the file it means.
/// 2. **The same name.** The collect that brought the audio along may
///    have flattened the folder structure, so only the file's name
///    matches. That is used only when exactly one file has the name —
///    an ambiguity is refused rather than guessed at.
fn find_sample_in_project(
    project_root: &Path,
    reference: &Path,
    filename: &str,
) -> Result<Option<PathBuf>, ClavusError> {
    // 1. The same path, found by its tail.
    let normalized = reference.to_string_lossy().replace('\\', "/");
    let parts: Vec<&str> = normalized
        .split('/')
        .filter(|p| !p.is_empty() && *p != "." && *p != "..")
        .collect();
    // The longest tail first: the most specific one that exists is
    // the one the reference means.
    for start in (0..parts.len()).rev() {
        let tail = parts[start..].join("/");
        let candidate = project_root.join(&tail);
        if candidate.is_file() {
            return Ok(Some(candidate));
        }
    }

    // 2. The same name, if there is exactly one file of that name —
    //    or if the files of that name all hold the same audio.
    let samples = project_root.join("Samples");
    if !samples.is_dir() {
        return Ok(None);
    }

    let mut found: Vec<PathBuf> = Vec::new();
    let mut stack = vec![samples];
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) => {
                error!("Failed to read {}: {}", dir.display(), e);
                continue;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            // A name that carries separators is a path in a name. Its
            // last part is the file's real name.
            let last = name.rsplit(['/', '\\']).next().unwrap_or(&name);
            if name == filename || last == filename {
                found.push(path);
            }
        }
    }

    if found.is_empty() {
        return Ok(None);
    }
    if found.len() == 1 {
        return Ok(found.pop());
    }

    // More than one file of that name. A collect run twice leaves the
    // same audio in two places, and then the choice does not matter:
    // the files hold the same bytes. Files that *differ* are a real
    // ambiguity, and that is refused rather than guessed at.
    if found.len() > 16 {
        return Ok(None);
    }
    let first = found[0].clone();
    for other in &found[1..] {
        if !same_bytes(&first, other) {
            return Ok(None);
        }
    }
    // The shallowest one, so the reference reads the way the set's own
    // folder does.
    Ok(found.into_iter().min_by_key(|p| p.components().count()))
}

/// A path relative to a root, with the root taken off.
///
/// The path is used as the file system has it. A file whose *name*
/// holds a separator — `Samples\Imported\loop.wav` — is one file, and
/// writing it with `/` instead would point at a file that is not
/// there.
fn relative_to(path: &Path, root: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(rel) => rel.to_string_lossy().to_string(),
        Err(_) => path.to_string_lossy().to_string(),
    }
}

/// Rewrite sample paths in an `.als` project file.
///
/// Decompresses the `.als`, replaces old paths with new relative paths in all
/// `<Path Value="..."/>` attributes, then recompresses.
///
/// **Atomic**: writes to a `.als.tmp` temp file first, then renames over the
/// original. If the process crashes mid-write, the original `.als` is intact.
///
/// Returns the paths that could not be rewritten. A reference that is not
/// found is not a success: the project still points at the outside file, and
/// the caller has to say so.
fn rewrite_project_sample_paths(
    project_path: &Path,
    replacements: HashMap<String, String>,
) -> Result<Vec<String>, ClavusError> {
    if replacements.is_empty() {
        return Ok(Vec::new());
    }

    // Read and decompress the .als
    let raw = fs::read(project_path)
        .map_err(|e| ClavusError::ParseError(format!("read {}: {}", project_path.display(), e)))?;

    let (xml_bytes, was_gzipped) = decompress_als_bytes(&raw)
        .map_err(|e| ClavusError::ParseError(format!("gzip decode {}: {}", project_path.display(), e)))?;

    let xml = String::from_utf8(xml_bytes)
        .map_err(ClavusError::from)?;

    // Replace old paths in Value="..." attributes of <Path> elements.
    //
    // The XML holds the path *escaped*: a file called `R&B/kick.wav` is
    // written `R&amp;B/kick.wav`. Matching only the raw path changed
    // nothing, and the tool still said the paths were rewritten — the
    // project went on playing the outside file. Both forms are matched,
    // and the new path is written escaped.
    let mut new_xml = xml.clone();
    let mut rewrite_count = 0;
    let mut unmatched: Vec<String> = Vec::new();

    for (old, new) in &replacements {
        if old.is_empty() || old == new {
            continue;
        }
        let old_escaped = crate::als_refs::encode_xml_entities(old);
        let new_escaped = crate::als_refs::encode_xml_entities(new);
        let mut matched = false;
        for (from, to) in [(&old_escaped, &new_escaped), (old, new)] {
            let attr = format!("Value=\"{}\"", from);
            if new_xml.contains(&attr) {
                let replacement = format!("Value=\"{}\"", to);
                new_xml = new_xml.replace(&attr, &replacement);
                matched = true;
                break;
            }
        }
        if matched {
            rewrite_count += 1;
            debug!("Rewrote: {} → {}", old, new);
        } else {
            debug!("No reference to {} found in {}", old, project_path.display());
            unmatched.push(old.clone());
        }
    }

    if rewrite_count == 0 {
        debug!("No path rewrites needed in {}", project_path.display());
        return Ok(unmatched);
    }

    info!("Rewrote {} sample paths in {}", rewrite_count, project_path.display());

    // Recompress and write atomically.
    // Preserve the original container format: a plain-XML .als stays plain
    // XML, a gzipped one is re-gzipped.
    let xml_out = new_xml.into_bytes();
    let payload = if was_gzipped {
        let mut compressed = Vec::new();
        {
            use flate2::write::GzEncoder;
            use flate2::Compression;
            let mut encoder = GzEncoder::new(&mut compressed, Compression::default());
            use std::io::Write;
            encoder.write_all(&xml_out)
                .map_err(|e| ClavusError::ParseError(format!("gzip write: {}", e)))?;
            encoder.finish()
                .map_err(|e| ClavusError::ParseError(format!("gzip finish: {}", e)))?;
        }
        compressed
    } else {
        xml_out
    };

    // Atomic write: temp file → rename
    //
    // A project that is a symlink is written *through* the link. The
    // rename would otherwise replace the link with a regular file, and
    // the user's project would quietly become a copy: the target would
    // still hold the old sample paths while the new file held the new
    // ones.
    let target = link_target_of(project_path);
    let tmp_path = target.with_extension("als.tmp");
    fs::write(&tmp_path, &payload)
        .map_err(|e| ClavusError::ParseError(format!("write temp {}: {}", tmp_path.display(), e)))?;
    std::fs::rename(&tmp_path, &target)
        .map_err(|e| ClavusError::ParseError(format!("rename {} → {}: {}", tmp_path.display(), target.display(), e)))?;

    Ok(unmatched)
}

/// The file a path really points at.
///
/// A symlink is followed to its target, so a rewrite lands in the file
/// the user actually keeps. A path that is not a symlink is returned
/// unchanged.
pub fn link_target_of(path: &Path) -> PathBuf {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return path.to_path_buf();
    };
    if !meta.file_type().is_symlink() {
        return path.to_path_buf();
    }
    match fs::canonicalize(path) {
        Ok(target) => target,
        Err(_) => path.to_path_buf(),
    }
}

// ─── Case-insensitive path resolution ───────────────────────────────────────

/// Resolve a relative path under `root`, matching each component
/// case-insensitively.
///
/// Live has been known to change the case of a folder in a saved set, and
/// on a case-sensitive filesystem the exact path then does not exist. This
/// walks the *same* path, only relaxing the case of each component — it
/// never accepts a different file that merely shares a name.
///
/// Is this path inside a macOS application bundle?
///
/// A Live set can use a drum from Live's own factory library, which
/// lives inside `Ableton Live 11 Suite.app`. That content belongs to
/// Live, not to the user, and it returns whenever Live is installed —
/// so it is not a missing sample and it must not stop a snapshot.
pub fn is_factory_library_path(path: &Path) -> bool {
    path.components().any(|c| {
        c.as_os_str()
            .to_string_lossy()
            .to_ascii_lowercase()
            .ends_with(".app")
    })
}

/// The places a machine's Ableton *user folder* is likely to be.
///
/// A set may point its samples at the user folder instead of at itself.
/// The set does not record where that folder was, so the usual places
/// are tried: whatever `CLAVUS_USER_FOLDER` names, the two standard
/// macOS places, and the volume the project itself sits on — because a
/// drive full of sets usually has its samples in the same place.
pub fn user_folder_roots(project_root: &Path) -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let extra = std::env::var_os("CLAVUS_USER_FOLDER").map(PathBuf::from);
    user_folder_roots_in(project_root, home.as_deref(), extra.as_deref())
}

/// The same, with the home folder and the user's own folder given.
///
/// They are parameters so the list can be tested without changing the
/// environment of the machine running the test.
pub fn user_folder_roots_in(
    project_root: &Path,
    home: Option<&Path>,
    extra: Option<&Path>,
) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut add = |p: PathBuf| {
        if !p.as_os_str().is_empty() && !roots.contains(&p) {
            roots.push(p);
        }
    };

    if let Some(dir) = extra {
        if dir.is_absolute() {
            add(dir.to_path_buf());
        }
    }

    if let Some(home) = home {
        // Live's own user library comes first. This is where Live
        // puts a user's own samples, and it is what a user-folder
        // reference is measured from — a set that says
        // `Samples/kick.wav` means this folder's `Samples/kick.wav`,
        // not `~/Music/Samples/kick.wav`. The folder is made when Live
        // is first run, so it may not be there yet.
        add(home.join("Music").join("Ableton").join("User Library"));
        add(home.join("Music"));
        add(home.join("Documents"));
    }

    // The volume the project is on: `/Volumes/LaCie`, or `/` for
    // anything under the home directory.
    if let Some(mount) = volume_root(project_root) {
        add(mount);
    }

    roots
}

/// The volume a path sits on: `/Volumes/LaCie` for a drive, `/` for
/// anything on the boot volume.
fn volume_root(path: &Path) -> Option<PathBuf> {
    let mut current = path;
    loop {
        let parent = current.parent()?;
        if parent == current {
            return None;
        }
        // A `Volumes` directory means *this* level is the mount point.
        if parent
            .file_name()
            .map(|n| n == "Volumes")
            .unwrap_or(false)
        {
            return Some(current.to_path_buf());
        }
        current = parent;
    }
}

/// Resolve a case-insensitive path, used when a Live set has changed the
/// case of a folder since the sample was recorded.
/// The archive entry that sits at exactly this relative path.
///
/// A bundle's own references are paths inside the bundle, so the entry
/// has to match the whole path and not just the file name: `samples/kick.wav`
/// and `loops/kick.wav` are two different samples that happen to share a
/// name.
fn archive_entry_at(archive_files: &[String], reference: &Path) -> Option<String> {
    let reference = reference.to_string_lossy().replace('\\', "/");
    archive_files
        .iter()
        .find(|entry| entry.replace('\\', "/").eq_ignore_ascii_case(&reference))
        .cloned()
}

fn resolve_case_insensitive(root: &Path, rel: &Path) -> Option<PathBuf> {
    let mut current = root.to_path_buf();
    for component in rel.components() {
        let name = component.as_os_str().to_str()?;
        let exact = current.join(name);
        if exact.exists() {
            current = exact;
            continue;
        }
        let mut matched = None;
        for entry in fs::read_dir(&current).ok()?.flatten() {
            if entry.file_name().to_string_lossy().eq_ignore_ascii_case(name) {
                matched = Some(entry.path());
                break;
            }
        }
        current = matched?;
    }
    if current.is_file() {
        Some(current)
    } else {
        None
    }
}

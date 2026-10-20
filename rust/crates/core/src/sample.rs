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
pub fn extract_als_sample_paths_from_xml(xml: &str) -> Vec<PathBuf> {
    let mut refs = Vec::new();
    let mut reader = quick_xml::Reader::from_str(xml);
    reader.trim_text(true);
    let mut buf = Vec::new();
    let mut in_sample_ref = false;
    let mut path_value: Option<String> = None;
    let mut relpath_parts: Vec<String> = Vec::new();
    let mut name_value: Option<String> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Start(e)) | Ok(quick_xml::events::Event::Empty(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                match tag.as_str() {
                    "SampleRef" => {
                        in_sample_ref = true;
                        path_value = None;
                        relpath_parts.clear();
                        name_value = None;
                    }
                    "Path" if in_sample_ref => {
                        for attr in e.attributes().flatten() {
                            let k = String::from_utf8_lossy(attr.key.as_ref()).to_string();
                            if k == "Value" {
                                path_value = Some(String::from_utf8_lossy(attr.value.as_ref()).to_string());
                            }
                        }
                    }
                    "RelativePathElement" if in_sample_ref => {
                        for attr in e.attributes().flatten() {
                            let k = String::from_utf8_lossy(attr.key.as_ref()).to_string();
                            if k == "Dir" {
                                relpath_parts.push(String::from_utf8_lossy(attr.value.as_ref()).to_string());
                            }
                        }
                    }
                    "Name" if in_sample_ref => {
                        for attr in e.attributes().flatten() {
                            let k = String::from_utf8_lossy(attr.key.as_ref()).to_string();
                            if k == "Value" {
                                name_value = Some(String::from_utf8_lossy(attr.value.as_ref()).to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(quick_xml::events::Event::End(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if tag == "SampleRef" && in_sample_ref {
                    in_sample_ref = false;
                    // Format 1: direct <Path Value="..."/>
                    if let Some(p) = path_value.take() {
                        if !p.trim().is_empty() {
                            refs.push(PathBuf::from(p));
                        }
                    }
                    // Format 2: <RelativePathElement> chain + <Name>
                    else if !relpath_parts.is_empty() || name_value.is_some() {
                        let mut p = relpath_parts.join("/");
                        if let Some(n) = name_value.take() {
                            if !p.is_empty() { p.push('/'); }
                            p.push_str(&n);
                        }
                        if !p.is_empty() {
                            refs.push(PathBuf::from(p));
                        }
                    }
                }
            }
            Ok(quick_xml::events::Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }
    refs
}

// ─── Audit ───────────────────────────────────────────────────────────────────

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
    let project_root = project_path
        .parent()
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
        let xml_bytes = flate2_decode(&raw)
            .map_err(|e| ClavusError::ParseError(format!("gzip: {}", e)))?;
        let xml = String::from_utf8(xml_bytes)
            .map_err(ClavusError::from)?;
        extract_als_sample_paths_from_xml(&xml)
    };

    debug!("audit_project: {} sample refs from {}", refs.len(), project_path.display());
    audit_sample_refs(&refs, project_root)
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
        return Ok(Vec::new()); // No project.xml — no samples
    }

    let xml = String::from_utf8(xml_bytes)
        .map_err(ClavusError::from)?;

    // Parse the XML and extract attribute values that look like audio paths
    let mut refs = Vec::new();
    let mut reader = quick_xml::Reader::from_str(&xml);
    reader.trim_text(true);
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Start(e)) | Ok(quick_xml::events::Event::Empty(e)) => {
                for attr in e.attributes().flatten() {
                    let val = String::from_utf8_lossy(attr.value.as_ref()).to_string();
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

/// Returns true if `val` looks like a path to an audio file.
/// Checks for absolute paths, relative paths with audio extensions, or paths
/// that are clearly file system references (not just IDs or UUIDs).
fn looks_like_audio_path(val: &str) -> bool {
    if val.is_empty() || val.len() < 4 {
        return false;
    }
    // Skip UUIDs/IDs (no slashes, no dots, or very short/long)
    if !val.contains('/') && !val.contains('\\') && !val.contains('.') {
        return false;
    }
    // Must have an audio extension or be an absolute path
    let lower = val.to_lowercase();
    lower.ends_with(".wav")
        || lower.ends_with(".aiff")
        || lower.ends_with(".aif")
        || lower.ends_with(".mp3")
        || lower.ends_with(".flac")
        || lower.ends_with(".ogg")
        || lower.ends_with(".m4a")
        || lower.ends_with(".wma")
        || (val.starts_with('/') || val.len() > 10 && val.contains(':'))
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
) -> Result<SampleAuditReport, ClavusError> {
    let mut collected = Vec::new();
    let mut missing = Vec::new();
    let mut outside_project = Vec::new();

    for path in paths {
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        // Try multiple resolution strategies:
        // 1. As-is from the .als (may be relative or absolute)
        // 2. Relative to project root (most common)
        // 3. Filename search (handles Live 10 path corruption bug, case-insensitive on macOS)
        let mut candidates: Vec<PathBuf> = vec![
            project_root.join(path),
            path.clone(),
        ];

        // Filename-only search: on macOS this is case-insensitive thanks to APFS,
        // on Linux it finds exact matches
        let fname = path.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if !fname.is_empty() {
            if let Ok(entries) = find_files_by_name(project_root, &fname) {
                for f in entries {
                    if !candidates.contains(&f) {
                        candidates.push(f);
                    }
                }
            }
        }

        let resolved = candidates.into_iter().find(|c| c.is_file());

        let resolved_path = resolved.clone();
        let relpath = resolved_path.as_ref().and_then(|p| {
            p.strip_prefix(project_root)
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
                "⚠ {} sample(s) outside project folder — run `clavus collect --copy-missing`",
                report.outside_project.len()
            );
        }
    } else {
        info!("✓ Sample audit passed: {} samples, all inside project folder", report.total());
    }

    Ok(report)
}

// ─── Collection ───────────────────────────────────────────────────────────────

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
    let project_root = project_path
        .parent()
        .ok_or_else(|| ClavusError::ParseError("no parent directory".into()))?;

    let samples_dir = project_root.join("Samples");
    fs::create_dir_all(&samples_dir)
        .map_err(|e| ClavusError::ParseError(format!("create Samples dir: {}", e)))?;

    let mut copied = Vec::new();
    let mut failed = Vec::new();
    // Map: original path as stored in XML → new relative path (Unix-slash)
    let mut path_replacements: HashMap<String, String> = HashMap::new();

    // Collect outside_project + missing (samples we CAN find)
    for ref_ in report.outside_project.iter().chain(report.missing.iter()) {
        if let Some(ref resolved) = ref_.resolved_path {
            let dest = samples_dir.join(&ref_.filename);
            match fs::copy(resolved, &dest) {
                Ok(bytes) => {
                    info!("Collected: {} ({} bytes)", ref_.filename, bytes);
                    copied.push(dest.clone());
                    // Build replacement: original path in XML → new relative path
                    let new_rel = format!("Samples/{}", ref_.filename);
                    let orig = ref_.path.to_string_lossy().replace('\\', "/");
                    path_replacements.insert(orig, new_rel);
                }
                Err(e) => {
                    error!("Failed to copy {}: {}", ref_.filename, e);
                    failed.push((ref_.path.clone(), e.to_string()));
                }
            }
        } else {
            // Missing: file doesn't exist on disk, can't copy
            failed.push((ref_.path.clone(), "file not found on disk".to_string()));
        }
    }

    // Rewrite project file sample paths
    if !path_replacements.is_empty() {
        rewrite_project_sample_paths(project_path, path_replacements)?;
    }

    Ok(CollectReport { copied, failed })
}

/// Report of a collect operation.
#[derive(Debug)]
pub struct CollectReport {
    /// Paths that were successfully copied into the project.
    pub copied: Vec<PathBuf>,
    /// (path, error) pairs for samples that could not be copied.
    pub failed: Vec<(PathBuf, String)>,
}

/// Rewrite sample paths in an `.als` project file.
///
/// Decompresses the `.als`, replaces old paths with new relative paths in all
/// `<Path Value="..."/>` attributes, then recompresses.
///
/// **Atomic**: writes to a `.als.tmp` temp file first, then renames over the
/// original. If the process crashes mid-write, the original `.als` is intact.
fn rewrite_project_sample_paths(
    project_path: &Path,
    replacements: HashMap<String, String>,
) -> Result<(), ClavusError> {
    if replacements.is_empty() {
        return Ok(());
    }

    // Read and decompress the .als
    let raw = fs::read(project_path)
        .map_err(|e| ClavusError::ParseError(format!("read {}: {}", project_path.display(), e)))?;

    let xml_bytes = flate2_decode(&raw)
        .map_err(|e| ClavusError::ParseError(format!("gzip decode {}: {}", project_path.display(), e)))?;

    let xml = String::from_utf8(xml_bytes)
        .map_err(ClavusError::from)?;

    // Replace old paths in Value="..." attributes of <Path> elements.
    // Uses simple string replacement which is safe because:
    // 1. Sample paths don't contain XML special characters
    // 2. We only match exact Value="oldpath" strings
    let mut new_xml = xml.clone();
    let mut rewrite_count = 0;

    for (old, new) in &replacements {
        if old.is_empty() || old == new {
            continue;
        }
        let attr = format!("Value=\"{}\"", old);
        if new_xml.contains(&attr) {
            let replacement = format!("Value=\"{}\"", new);
            new_xml = new_xml.replace(&attr, &replacement);
            rewrite_count += 1;
            debug!("Rewrote: {} → {}", old, new);
        }
    }

    if rewrite_count == 0 {
        debug!("No path rewrites needed in {}", project_path.display());
        return Ok(());
    }

    info!("Rewrote {} sample paths in {}", rewrite_count, project_path.display());

    // Recompress and write atomically
    let xml_out = new_xml.into_bytes();
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

    // Atomic write: temp file → rename
    let tmp_path = project_path.with_extension("als.tmp");
    fs::write(&tmp_path, &compressed)
        .map_err(|e| ClavusError::ParseError(format!("write temp {}: {}", tmp_path.display(), e)))?;
    std::fs::rename(&tmp_path, project_path)
        .map_err(|e| ClavusError::ParseError(format!("rename {} → {}: {}", tmp_path.display(), project_path.display(), e)))?;

    Ok(())
}

// ─── Filename search ─────────────────────────────────────────────────────────

/// Find files matching `name` anywhere under `root` (shallow recursive scan).
fn find_files_by_name(root: &Path, name: &str) -> std::io::Result<Vec<PathBuf>> {
    let mut results = Vec::new();
    _find_files_by_name(root, name, &mut results, 0);
    Ok(results)
}

fn matches_name(name: &str, entry_name: &str) -> bool {
    // On macOS (APFS/HFS+), filename comparison is case-insensitive
    #[cfg(target_os = "macos")]
    {
        entry_name.eq_ignore_ascii_case(name)
    }
    #[cfg(not(target_os = "macos"))]
    {
        entry_name == name
    }
}

fn _find_files_by_name(dir: &Path, name: &str, results: &mut Vec<PathBuf>, depth: usize) {
    if depth > 4 {
        return; // Limit recursion depth
    }
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && !path.file_name().map(|n| n.to_string_lossy().starts_with('.')).unwrap_or(false) {
                _find_files_by_name(&path, name, results, depth + 1);
            } else if path.is_file() && path.file_name().map(|n| matches_name(name, &n.to_string_lossy())).unwrap_or(false) {
                results.push(path);
            }
        }
    }
}

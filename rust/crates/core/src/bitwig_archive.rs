//! Reading and rewriting the contents of a Bitwig `.bwproject` archive.
//!
//! A `.bwproject` is a ZIP archive containing `project.xml` and, often,
//! a copy of every sample the project uses. When such an archive is
//! shared, the sample references inside `project.xml` still point at the
//! original machine's absolute paths — so on the receiving machine the
//! samples look missing even though they are right there in the archive.
//!
//! This module lets the audit see what the archive holds, and lets
//! `collect` extract those samples and rewrite the references.

use crate::error::ClavusError;
use std::fs;
use std::io::{Read, Write};
use std::path::Path;

/// The name of the project description inside a `.bwproject` archive.
pub const PROJECT_XML: &str = "project.xml";

/// Every file name in the archive.
pub fn list_entries(project_path: &Path) -> Result<Vec<String>, ClavusError> {
    let file = fs::File::open(project_path)
        .map_err(|e| ClavusError::ParseError(format!("open {}: {}", project_path.display(), e)))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| ClavusError::ParseError(format!("zip open: {}", e)))?;

    let mut names = Vec::with_capacity(archive.len());
    for i in 0..archive.len() {
        let entry = archive
            .by_index(i)
            .map_err(|e| ClavusError::ParseError(format!("zip entry: {}", e)))?;
        names.push(entry.name().to_string());
    }
    Ok(names)
}

/// The entry that holds the project description, if there is one.
pub fn find_project_xml(entries: &[String]) -> Option<String> {
    entries
        .iter()
        .find(|n| {
            *n == PROJECT_XML
                || n.ends_with(&format!("/{}", PROJECT_XML))
                || n.ends_with(&format!("\\{}", PROJECT_XML))
        })
        .cloned()
}

/// Read one file out of the archive.
pub fn read_entry(project_path: &Path, name: &str) -> Result<Vec<u8>, ClavusError> {
    let file = fs::File::open(project_path)
        .map_err(|e| ClavusError::ParseError(format!("open {}: {}", project_path.display(), e)))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| ClavusError::ParseError(format!("zip open: {}", e)))?;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| ClavusError::ParseError(format!("zip entry: {}", e)))?;
        if entry.name() == name {
            let mut buf = Vec::new();
            entry
                .read_to_end(&mut buf)
                .map_err(|e| ClavusError::ParseError(format!("read {}: {}", name, e)))?;
            return Ok(buf);
        }
    }
    Err(ClavusError::ParseError(format!(
        "{} is not in {}",
        name,
        project_path.display()
    )))
}

/// The archive entry holding a file with the given base name.
///
/// Bitwig's exported archives keep the samples under a `samples/`
/// folder, but the exact layout is not guaranteed, so any entry whose
/// file name matches counts.
pub fn find_entry_by_filename(entries: &[String], filename: &str) -> Option<String> {
    if filename.is_empty() {
        return None;
    }
    entries
        .iter()
        .find(|n| {
            let base = n.rsplit(['/', '\\']).next().unwrap_or(n);
            base.eq_ignore_ascii_case(filename)
        })
        .cloned()
}

/// Replace one file inside the archive.
///
/// The archive is written to a temporary file and renamed over the
/// original, so a failure cannot leave a half-written project behind.
/// Every other entry is copied across unchanged.
pub fn replace_entry(
    project_path: &Path,
    name: &str,
    contents: &[u8],
) -> Result<(), ClavusError> {
    let file = fs::File::open(project_path)
        .map_err(|e| ClavusError::ParseError(format!("open {}: {}", project_path.display(), e)))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| ClavusError::ParseError(format!("zip open: {}", e)))?;

    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let mut replaced = false;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| ClavusError::ParseError(format!("zip entry: {}", e)))?;
        let entry_name = entry.name().to_string();
        let is_dir = entry.is_dir();

        // The zip format cannot store a year before 1980, so an
        // out-of-range timestamp falls back to the default.
        let options = entry
            .last_modified()
            .and_then(|t| {
                zip::DateTime::from_date_and_time(
                    t.year().max(1980),
                    t.month().max(1),
                    t.day().max(1),
                    t.hour(),
                    t.minute(),
                    t.second(),
                )
                .ok()
            })
            .map(|dt| zip::write::SimpleFileOptions::default().last_modified_time(dt))
            .unwrap_or_default();

        if entry_name == name {
            out.start_file(&entry_name, options)
                .map_err(|e| ClavusError::ParseError(format!("zip write: {}", e)))?;
            out.write_all(contents)
                .map_err(|e| ClavusError::ParseError(format!("zip write: {}", e)))?;
            replaced = true;
        } else if is_dir {
            out.add_directory(&entry_name, options)
                .map_err(|e| ClavusError::ParseError(format!("zip write: {}", e)))?;
        } else {
            let mut buf = Vec::new();
            entry
                .read_to_end(&mut buf)
                .map_err(|e| ClavusError::ParseError(format!("read {}: {}", entry_name, e)))?;
            out.start_file(&entry_name, options)
                .map_err(|e| ClavusError::ParseError(format!("zip write: {}", e)))?;
            out.write_all(&buf)
                .map_err(|e| ClavusError::ParseError(format!("zip write: {}", e)))?;
        }
    }

    if !replaced {
        return Err(ClavusError::ParseError(format!(
            "{} is not in {}",
            name,
            project_path.display()
        )));
    }

    let cursor = out
        .finish()
        .map_err(|e| ClavusError::ParseError(format!("zip finish: {}", e)))?;

    // Atomic write: temp file → rename
    let tmp_path = project_path.with_extension("bwproject.tmp");
    fs::write(&tmp_path, cursor.into_inner())
        .map_err(|e| ClavusError::ParseError(format!("write temp {}: {}", tmp_path.display(), e)))?;
    fs::rename(&tmp_path, project_path).map_err(|e| {
        let _ = fs::remove_file(&tmp_path);
        ClavusError::ParseError(format!("rename {}: {}", project_path.display(), e))
    })?;

    Ok(())
}

/// Rewrite a sample path inside the archive's project description.
///
/// The reference is matched as a whole attribute value, so a path that
/// happens to be a suffix of another one is not touched.
pub fn rewrite_sample_path(
    project_path: &Path,
    old_path: &str,
    new_path: &str,
) -> Result<bool, ClavusError> {
    if old_path.is_empty() || old_path == new_path {
        return Ok(false);
    }

    let entries = list_entries(project_path)?;
    let Some(xml_name) = find_project_xml(&entries) else {
        return Ok(false);
    };

    let xml_bytes = read_entry(project_path, &xml_name)?;
    let xml = String::from_utf8(xml_bytes).map_err(ClavusError::from)?;

    let old_attr = format!("\"{}\"", old_path);
    let new_attr = format!("\"{}\"", new_path);
    if !xml.contains(&old_attr) {
        return Ok(false);
    }

    let new_xml = xml.replace(&old_attr, &new_attr);
    replace_entry(project_path, &xml_name, new_xml.as_bytes())?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_archive(path: &Path, entries: &[(&str, &[u8])]) {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        for (name, data) in entries {
            w.start_file(*name, opts).unwrap();
            w.write_all(data).unwrap();
        }
        let cursor = w.finish().unwrap();
        fs::write(path, cursor.into_inner()).unwrap();
    }

    #[test]
    fn lists_and_reads_entries() {
        let dir = std::env::temp_dir().join(format!("bwarc-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("p.bwproject");
        make_archive(
            &path,
            &[
                ("project.xml", b"<Project/>"),
                ("samples/kick.wav", b"RIFFfake"),
            ],
        );

        let entries = list_entries(&path).unwrap();
        assert!(entries.contains(&"project.xml".to_string()));
        assert!(find_project_xml(&entries).is_some());
        assert_eq!(read_entry(&path, "samples/kick.wav").unwrap(), b"RIFFfake");
        assert!(read_entry(&path, "nope.wav").is_err());

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn finds_an_entry_by_file_name() {
        let entries = vec![
            "project.xml".to_string(),
            "samples/kick.wav".to_string(),
        ];
        assert_eq!(
            find_entry_by_filename(&entries, "kick.wav"),
            Some("samples/kick.wav".to_string())
        );
        assert_eq!(find_entry_by_filename(&entries, "KICK.WAV"), Some("samples/kick.wav".to_string()));
        assert_eq!(find_entry_by_filename(&entries, "bass.wav"), None);
    }

    #[test]
    fn replaces_an_entry_and_keeps_the_rest() {
        let dir = std::env::temp_dir().join(format!("bwarc2-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("p.bwproject");
        make_archive(
            &path,
            &[
                ("project.xml", b"<Project File=\"/old/kick.wav\"/>"),
                ("samples/kick.wav", b"RIFFfake"),
            ],
        );

        assert!(rewrite_sample_path(&path, "/old/kick.wav", "Samples/kick.wav").unwrap());

        let entries = list_entries(&path).unwrap();
        assert!(entries.contains(&"samples/kick.wav".to_string()));
        let xml = read_entry(&path, "project.xml").unwrap();
        assert!(String::from_utf8_lossy(&xml).contains("Samples/kick.wav"));
        assert_eq!(read_entry(&path, "samples/kick.wav").unwrap(), b"RIFFfake");

        fs::remove_dir_all(&dir).ok();
    }
}

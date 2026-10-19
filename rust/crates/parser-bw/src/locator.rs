//! Inject Clavus cues as Bitwig locators.
//!
//! Modifies the `<Locator>` elements inside the zipped `project.xml`.

use clavus_core::types::{Cue, CueStatus};
use std::io::{Read, Write};
use tracing::info;
use zip::ZipArchive;

/// Inject cue locators into a `.bwproject` ZIP.
/// Returns the modified `.bwproject` bytes.
pub fn inject_locators(
    bwproject_bytes: &[u8],
    cues: &[Cue],
) -> Result<Vec<u8>, clavus_core::ClavusError> {
    let unresolved: Vec<&Cue> = cues
        .iter()
        .filter(|c| c.status == CueStatus::Pending || c.status == CueStatus::Deferred)
        .collect();

    if unresolved.is_empty() {
        return Ok(bwproject_bytes.to_vec());
    }

    // Open the ZIP
    let reader = std::io::Cursor::new(bwproject_bytes);
    let mut archive = ZipArchive::new(reader)
        .map_err(|e| clavus_core::ClavusError::ParseError(format!("zip open: {}", e)))?;

    // Find project.xml in the archive
    let project_xml_idx = (0..archive.len())
        .find(|i| {
            archive.by_index(*i)
                .map(|e| e.name() == "project.xml" || e.name().ends_with("/project.xml"))
                .unwrap_or(false)
        })
        .ok_or_else(|| clavus_core::ClavusError::ParseError("No project.xml found".into()))?;

    let mut project_xml_bytes = Vec::new();
    archive.by_index(project_xml_idx)
        .map_err(|e| clavus_core::ClavusError::ParseError(format!("read project.xml: {}", e)))?
        .read_to_end(&mut project_xml_bytes)
        .map_err(|e| clavus_core::ClavusError::ParseError(format!("read project.xml: {}", e)))?;

    let xml = String::from_utf8(project_xml_bytes.clone())
        .map_err(clavus_core::ClavusError::Utf8Error)?;

    // Build new locator XML
    let mut new_locators = String::new();
    for cue in &unresolved {
        let name = format!("● {}", &cue.text[..cue.text.len().min(60)]);
        new_locators.push_str(&format!(
            r#"<Locator Time="{}" Name="{}"/>"#,
            cue.position, name
        ));
    }

    // Inject into the Locators section
    let modified_xml = if let Some(pos) = xml.find("<Locators>") {
        let insert_pos = pos + "<Locators>".len();
        format!("{}{}{}", &xml[..insert_pos], new_locators, &xml[insert_pos..])
    } else {
        xml
    };

    // Rebuild the ZIP with modified project.xml
    let mut output = Vec::new();
    {
        use zip::write::ZipWriter;
        use zip::CompressionMethod;
        let mut zip = ZipWriter::new(std::io::Cursor::new(&mut output));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated);

        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)
                .map_err(|e| clavus_core::ClavusError::ParseError(e.to_string()))?;
            let name = entry.name().to_string();
            let entry_compression = entry.compression();

            if i == project_xml_idx {
                zip.start_file(&name, options)
                    .map_err(|e| clavus_core::ClavusError::ParseError(e.to_string()))?;
                zip.write_all(modified_xml.as_bytes())
                    .map_err(|e| clavus_core::ClavusError::ParseError(e.to_string()))?;
            } else {
                let mut buf = Vec::new();
                entry.read_to_end(&mut buf)
                    .map_err(|e| clavus_core::ClavusError::ParseError(e.to_string()))?;
                let entry_options = zip::write::SimpleFileOptions::default()
                    .compression_method(entry_compression);
                zip.start_file(&name, entry_options)
                    .map_err(|e| clavus_core::ClavusError::ParseError(e.to_string()))?;
                zip.write_all(&buf)
                    .map_err(|e| clavus_core::ClavusError::ParseError(e.to_string()))?;
            }
        }
        zip.finish()
            .map_err(|e| clavus_core::ClavusError::ParseError(e.to_string()))?;
    }

    info!("Injected {} locators into .bwproject", unresolved.len());
    Ok(output)
}

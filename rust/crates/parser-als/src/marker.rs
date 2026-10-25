//! Inject Clavus cues as Ableton markers.
//!
//! Modifies the `<CuePoints>` section in the `.als` XML.
//! Supports both Live 10/11 (`<CuePoints>`) and Live 12 (`<Locators>`) formats.
//! Creates a backup `.als.bak` before modifying.

use clavus_core::types::{Cue, CueStatus};
use std::path::Path;
use tracing::info;

/// Inject cue markers into an `.als` file.
/// Modifies the `<CuePoints>` (Live 10/11) or `<Locators>` (Live 12) section.
/// Returns the modified `.als` bytes (gzipped).
/// Creates a backup before modifying.
pub fn inject_markers(
    als_bytes: &[u8],
    cues: &[Cue],
    inject_into: Option<&Path>,
) -> Result<Vec<u8>, clavus_core::ClavusError> {
    let unresolved: Vec<&Cue> = cues
        .iter()
        .filter(|c| c.status == CueStatus::Pending || c.status == CueStatus::Deferred)
        .collect();

    if unresolved.is_empty() {
        return Ok(als_bytes.to_vec());
    }

    let als_path = inject_into
        .ok_or_else(|| clavus_core::ClavusError::ParseError("no inject target".into()))?;

    // Create backup if it doesn't exist
    let backup_path = als_path.with_extension("als.bak");
    if !backup_path.exists() {
        std::fs::copy(als_path, &backup_path)
            .map_err(|e| clavus_core::ClavusError::ParseError(format!("backup: {}", e)))?;
        info!("Created backup: {}", backup_path.display());
    }

    // Decompress gzip
    let xml_bytes = flate2_decode(als_bytes)
        .map_err(|e| clavus_core::ClavusError::ParseError(format!("gzip: {}", e)))?;

    let xml = String::from_utf8(xml_bytes)
        .map_err(clavus_core::ClavusError::Utf8Error)?;

    // Detect format: Live 12 has <Locators>, older has <CuePoints>
    let is_live_12 = xml.contains("<Locators>");

    // Build new marker XML elements
    let mut new_markers = String::new();
    for cue in &unresolved {
        // The name is cut at a character boundary and escaped, so a
        // cue with an accent in it does not panic and a cue with a
        // quote in it does not break the set.
        let marker_name = clavus_core::marker_name(&cue.text);
        let position = clavus_core::als_refs::encode_xml_entities(&cue.position);
        if is_live_12 {
            // Live 12: <Locator Time="beats" Name="..."/>
            new_markers.push_str(&format!(
                r#"<Locator Time="{}" Name="{}"/>"#,
                position, marker_name
            ));
        } else {
            // Live 10/11: <CuePoint Time="bar.beat.ticks" Name="..."/>
            new_markers.push_str(&format!(
                r#"<CuePoint><Time Value="{}"/><Name Value="{}"/></CuePoint>"#,
                position, marker_name
            ));
        }
    }

    // Inject into XML
    let modified_xml = if is_live_12 {
        // Find <Locators><Locators> and inject
        if let Some(pos) = xml.find("<Locators>") {
            let after = &xml[pos + "<Locators>".len()..];
            if let Some(inner_pos) = after.find("<Locators>") {
                let insert_pos = pos + "<Locators>".len() + inner_pos + "<Locators>".len();
                format!("{}{}{}", &xml[..insert_pos], new_markers, &xml[insert_pos..])
            } else {
                // No inner Locators — insert after opening <Locators>
                format!("{}{}{}", &xml[..pos + "<Locators>".len()], new_markers, &xml[pos + "<Locators>".len()..])
            }
        } else {
            xml // No Locators element — can't inject
        }
    } else {
        // Find <CuePoints> and inject
        if let Some(pos) = xml.find("<CuePoints>") {
            let insert_pos = pos + "<CuePoints>".len();
            format!("{}{}{}", &xml[..insert_pos], new_markers, &xml[insert_pos..])
        } else {
            xml // No CuePoints element — can't inject
        }
    };

    // Re-compress gzip
    let modified_bytes = flate2_encode(modified_xml.as_bytes());

    info!("Injected {} markers into {}", unresolved.len(), als_path.display());
    Ok(modified_bytes)
}

fn flate2_decode(data: &[u8]) -> std::io::Result<Vec<u8>> {
    use flate2::read::GzDecoder;
    use std::io::Read;
    let mut decoder = GzDecoder::new(data);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out)?;
    Ok(out)
}

fn flate2_encode(data: &[u8]) -> Vec<u8> {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

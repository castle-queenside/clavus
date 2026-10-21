//! Extract audio sample paths from an `.als` XML file.
//!
//! The extraction lives in [`clavus_core::als_refs`] so that the Ableton
//! parser and the sample audit can never disagree about what a Live set
//! references — a disagreement there means a sample is silently treated
//! as missing.

use std::io::Read;
use std::path::PathBuf;

/// Extract all audio sample paths from an `.als` file.
/// Returns paths as stored in the XML (may be relative or absolute).
pub fn extract_sample_refs(als_path: &std::path::Path) -> Result<Vec<PathBuf>, clavus_core::ClavusError> {
    let raw = std::fs::read(als_path)
        .map_err(|e| clavus_core::ClavusError::ParseError(format!("read {}: {}", als_path.display(), e)))?;

    let xml_bytes = flate2_decode(&raw)
        .map_err(|e| clavus_core::ClavusError::ParseError(format!("gzip: {}", e)))?;

    let xml = String::from_utf8(xml_bytes)
        .map_err(clavus_core::ClavusError::Utf8Error)?;

    Ok(clavus_core::als_refs::extract_sample_refs_from_xml(&xml))
}

fn flate2_decode(data: &[u8]) -> std::io::Result<Vec<u8>> {
    use flate2::read::GzDecoder;
    let mut decoder = GzDecoder::new(data);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out)?;
    Ok(out)
}

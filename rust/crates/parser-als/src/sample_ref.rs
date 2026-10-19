//! Extract audio sample paths from an `.als` XML file.
//!
//! Handles two formats used by different Live versions:
//! 1. **Live 12+**: `<SampleRef><Path Value="relative/path.wav"/></SampleRef>`
//! 2. **Older Live**: `<SampleRef>` with `<RelativePathElement Dir="..."/>` chain
//!    + `<Name Value="filename.wav"/>`

use std::io::Read;
use std::path::PathBuf;
use quick_xml::events::Event;
use quick_xml::Reader;
use tracing::debug;

/// Extract all audio sample paths from an `.als` file.
/// Returns paths as stored in the XML (may be relative or absolute).
pub fn extract_sample_refs(als_path: &std::path::Path) -> Result<Vec<PathBuf>, clavus_core::ClavusError> {
    let raw = std::fs::read(als_path)
        .map_err(|e| clavus_core::ClavusError::ParseError(format!("read {}: {}", als_path.display(), e)))?;

    let xml_bytes = flate2_decode(&raw)
        .map_err(|e| clavus_core::ClavusError::ParseError(format!("gzip: {}", e)))?;

    let xml = String::from_utf8(xml_bytes)
        .map_err(clavus_core::ClavusError::Utf8Error)?;

    let mut refs = Vec::new();
    let mut reader = Reader::from_str(&xml);
    reader.trim_text(true);
    let mut buf = Vec::new();
    let mut in_sample_ref = false;
    let mut path_value: Option<String> = None;
    let mut relpath_parts: Vec<String> = Vec::new();
    let mut name_value: Option<String> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
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
            Ok(Event::End(e)) => {
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
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }

    debug!("Extracted {} sample refs from {}", refs.len(), als_path.display());
    Ok(refs)
}

fn flate2_decode(data: &[u8]) -> std::io::Result<Vec<u8>> {
    use flate2::read::GzDecoder;
    let mut decoder = GzDecoder::new(data);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out)?;
    Ok(out)
}

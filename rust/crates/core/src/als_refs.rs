//! Audio sample references in an Ableton Live set.
//!
//! Live has changed the way a sample is recorded several times:
//!
//! - **Live 12+**: `<SampleRef><Path Value="Samples/kick.wav"/></SampleRef>`
//! - **Live 10/11**: the same, sometimes wrapped in a second `<SampleRef>`
//! - **Live 9**: `<SampleRef><FileRef>` holding either a project-relative
//!   `<RelativePath>` chain or a library-relative
//!   `<SearchHint><PathHint>` chain, plus `<Name Value="filename.wav"/>`
//!
//! Two Live 9 traps bit real users:
//!
//! 1. A `SampleRef` carries **two** `FileRef` elements and repeats the
//!    `PathHint` chain in both. Collecting the chain from both glued them
//!    together, producing paths like
//!    `Studio/Acapellas/X/Studio/Acapellas/X/file.mp3` that no user can
//!    act on. Only the first `FileRef` of a `SampleRef` is read.
//! 2. The `PathHint` chain is a *search hint* — where the file was last
//!    seen — and must never be mixed with the real `RelativePath` chain.
//!
//! XML entities are decoded as well, so a sample called `A & B <mix>.wav`
//! is looked up under its real name and not under the raw XML text.

use std::path::PathBuf;
use quick_xml::events::Event;
use quick_xml::Reader;

/// Check that an XML document is complete.
///
/// A well-formed XML document always ends with `>` — the last thing in it
/// is a closing tag, an empty element, a comment or a processing
/// instruction.
///
/// This check matters because the XML reader accepts a document whose
/// last tag is missing its `>`: `</LiveSet` is read as a closing tag, so
/// a Live set truncated by a single byte parsed as a *complete* set with
/// every track still in it. The set was then snapshotted as if it were
/// healthy.
pub fn is_complete_document(xml: &str) -> bool {
    xml.trim_end().ends_with('>')
}

/// Extract every audio sample path recorded in a Live set's XML.
pub fn extract_sample_refs_from_xml(xml: &str) -> Vec<PathBuf> {
    let mut refs = Vec::new();
    let mut reader = Reader::from_str(xml);
    reader.trim_text(true);
    let mut buf = Vec::new();

    // Stack of open element names, so a `RelativePathElement` can be
    // attributed to its real parent.
    let mut stack: Vec<String> = Vec::new();

    let mut in_sample_ref = false;
    // State of the `FileRef` currently being read.
    let mut path_value: Option<String> = None;
    let mut relpath_parts: Vec<String> = Vec::new();
    let mut hint_parts: Vec<String> = Vec::new();
    let mut name_value: Option<String> = None;
    let mut has_relative_path: Option<bool> = None;
    // A `SampleRef` may carry several `FileRef`s; only the first one that
    // yields a path is used.
    let mut produced = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                match tag.as_str() {
                    "SampleRef" => {
                        in_sample_ref = true;
                        produced = false;
                        reset_fileref(
                            &mut path_value,
                            &mut relpath_parts,
                            &mut hint_parts,
                            &mut name_value,
                            &mut has_relative_path,
                        );
                    }
                    "FileRef" if in_sample_ref => {
                        reset_fileref(
                            &mut path_value,
                            &mut relpath_parts,
                            &mut hint_parts,
                            &mut name_value,
                            &mut has_relative_path,
                        );
                    }
                    "Path" if in_sample_ref => {
                        if let Some(v) = attr_value(&e, "Value") {
                            path_value = Some(v);
                        }
                    }
                    "HasRelativePath" if in_sample_ref => {
                        if let Some(v) = attr_value(&e, "Value") {
                            has_relative_path = Some(matches!(
                                v.trim().to_ascii_lowercase().as_str(),
                                "true" | "1"
                            ));
                        }
                    }
                    "RelativePathElement" if in_sample_ref => {
                        if let Some(v) = attr_value(&e, "Dir") {
                            match stack.last().map(|s| s.as_str()) {
                                // The real, project-relative path.
                                Some("RelativePath") => relpath_parts.push(v),
                                // A search hint — where the file was last seen.
                                Some("PathHint") => hint_parts.push(v),
                                _ => {}
                            }
                        }
                    }
                    "Name" if in_sample_ref => {
                        if let Some(v) = attr_value(&e, "Value") {
                            name_value = Some(v);
                        }
                    }
                    _ => {}
                }
                stack.push(tag);
            }
            Ok(Event::Empty(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                match tag.as_str() {
                    "Path" if in_sample_ref => {
                        if let Some(v) = attr_value(&e, "Value") {
                            path_value = Some(v);
                        }
                    }
                    "HasRelativePath" if in_sample_ref => {
                        if let Some(v) = attr_value(&e, "Value") {
                            has_relative_path = Some(matches!(
                                v.trim().to_ascii_lowercase().as_str(),
                                "true" | "1"
                            ));
                        }
                    }
                    "RelativePathElement" if in_sample_ref => {
                        if let Some(v) = attr_value(&e, "Dir") {
                            match stack.last().map(|s| s.as_str()) {
                                Some("RelativePath") => relpath_parts.push(v),
                                Some("PathHint") => hint_parts.push(v),
                                _ => {}
                            }
                        }
                    }
                    "Name" if in_sample_ref => {
                        if let Some(v) = attr_value(&e, "Value") {
                            name_value = Some(v);
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if stack.last().map(|s| s.as_str()) == Some(tag.as_str()) {
                    stack.pop();
                }
                if tag == "FileRef" && in_sample_ref && !produced {
                    if let Some(p) = build_path(
                        &path_value,
                        &relpath_parts,
                        &hint_parts,
                        &name_value,
                        has_relative_path,
                    ) {
                        refs.push(PathBuf::from(p));
                        produced = true;
                    }
                } else if tag == "SampleRef" {
                    // A Live 12 set has no `FileRef` at all — the path sits
                    // directly in the `SampleRef`.
                    if !produced {
                        if let Some(p) = build_path(
                            &path_value,
                            &relpath_parts,
                            &hint_parts,
                            &name_value,
                            has_relative_path,
                        ) {
                            refs.push(PathBuf::from(p));
                            produced = true;
                        }
                    }
                    in_sample_ref = false;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    refs
}

/// Clear the per-`FileRef` state.
fn reset_fileref(
    path_value: &mut Option<String>,
    relpath_parts: &mut Vec<String>,
    hint_parts: &mut Vec<String>,
    name_value: &mut Option<String>,
    has_relative_path: &mut Option<bool>,
) {
    *path_value = None;
    relpath_parts.clear();
    hint_parts.clear();
    *name_value = None;
    *has_relative_path = None;
}

/// Build the sample path for one `FileRef`.
///
/// Live 12+ stores the path directly. Live 9 stores either a
/// project-relative path (`HasRelativePath` true) or a library-relative
/// search hint — never both, and never the two glued together.
fn build_path(
    path_value: &Option<String>,
    relpath_parts: &[String],
    hint_parts: &[String],
    name_value: &Option<String>,
    has_relative_path: Option<bool>,
) -> Option<String> {
    // Live 12+: the path is right there.
    if let Some(p) = path_value {
        let p = p.trim();
        if !p.is_empty() {
            return Some(p.to_string());
        }
    }

    let name = name_value.as_ref().map(|n| n.trim()).unwrap_or("");

    // Live 9 with a project-relative path.
    if has_relative_path == Some(true) && !relpath_parts.is_empty() {
        return Some(join_parts(relpath_parts, name));
    }

    // Live 9 with a library-relative search hint. This is where the file
    // was last seen, which is the only path the set records.
    if !hint_parts.is_empty() {
        return Some(join_parts(hint_parts, name));
    }

    if !relpath_parts.is_empty() {
        return Some(join_parts(relpath_parts, name));
    }

    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// Join path components and a file name with `/`.
fn join_parts(parts: &[String], name: &str) -> String {
    let mut p = parts.join("/");
    if !name.is_empty() {
        if !p.is_empty() {
            p.push('/');
        }
        p.push_str(name);
    }
    p
}

/// Read an attribute value, decoding XML entities.
fn attr_value(e: &quick_xml::events::BytesStart, key: &str) -> Option<String> {
    for attr in e.attributes().flatten() {
        let k = String::from_utf8_lossy(attr.key.as_ref()).to_string();
        if k == key {
            let raw = String::from_utf8_lossy(attr.value.as_ref()).to_string();
            return Some(decode_xml_entities(&raw));
        }
    }
    None
}

/// Decode the XML entities that can appear in an attribute value.
///
/// `&amp;` is decoded last so that `&amp;lt;` becomes the literal text
/// `&lt;` rather than `<`.
pub fn decode_xml_entities(input: &str) -> String {
    if !input.contains('&') {
        return input.to_string();
    }
    let mut out = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '&' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        // Find the terminating ';' within a sane distance.
        let end = (i + 1..chars.len().min(i + 12)).find(|&j| chars[j] == ';');
        let Some(end) = end else {
            out.push('&');
            i += 1;
            continue;
        };
        let entity: String = chars[i + 1..end].iter().collect();
        let replacement = match entity.as_str() {
            "lt" => Some("<".to_string()),
            "gt" => Some(">".to_string()),
            "quot" => Some("\"".to_string()),
            "apos" => Some("'".to_string()),
            "amp" => Some("&".to_string()),
            _ => numeric_entity(&entity),
        };
        match replacement {
            Some(r) => {
                out.push_str(&r);
                i = end + 1;
            }
            None => {
                out.push('&');
                i += 1;
            }
        }
    }
    out
}

/// Encode the characters that cannot appear raw in an XML attribute.
///
/// The inverse of `decode_xml_entities`. `&` is encoded first, so a
/// file name that literally contains `&amp;` becomes `&amp;amp;` —
/// which is exactly what the decoder turns back into `&amp;`.
///
/// A rewrite that writes a raw path back into the XML breaks the
/// document: an unescaped `&` is not valid XML, and a path with a
/// `<` in it truncates the attribute.
pub fn encode_xml_entities(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// Decode `&#NN;` and `&#xNN;` entities.
fn numeric_entity(entity: &str) -> Option<String> {
    let digits = entity.strip_prefix('#')?;
    let code = if let Some(hex) = digits.strip_prefix('x').or_else(|| digits.strip_prefix('X')) {
        u32::from_str_radix(hex, 16).ok()?
    } else {
        digits.parse::<u32>().ok()?
    };
    char::from_u32(code).map(|c| c.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_xml_entities() {
        assert_eq!(decode_xml_entities("A &amp; B"), "A & B");
        assert_eq!(decode_xml_entities("&lt;mix&gt;"), "<mix>");
        assert_eq!(decode_xml_entities("&quot;q&quot;"), "\"q\"");
        assert_eq!(decode_xml_entities("&#65;&#x42;"), "AB");
        // An unknown entity is left alone.
        assert_eq!(decode_xml_entities("&nope;"), "&nope;");
        // A double-encoded entity decodes once, not twice.
        assert_eq!(decode_xml_entities("&amp;lt;"), "&lt;");
    }

    #[test]
    fn live12_path_is_used_verbatim() {
        let xml = r#"<Ableton><LiveSet><SampleRef><Path Value="Samples/kick.wav"/></SampleRef></LiveSet></Ableton>"#;
        let refs = extract_sample_refs_from_xml(xml);
        assert_eq!(refs, vec![PathBuf::from("Samples/kick.wav")]);
    }

    #[test]
    fn live9_search_hint_is_not_duplicated() {
        // A Live 9 SampleRef carries two FileRefs, each with the same
        // PathHint. The path must appear once, not glued twice.
        let xml = r#"<Ableton><LiveSet><SampleRef>
            <FileRef>
                <HasRelativePath Value="false" />
                <RelativePath />
                <Name Value="B08.mp3" />
                <SearchHint><PathHint>
                    <RelativePathElement Dir="Studio" />
                    <RelativePathElement Dir="Acapellas" />
                </PathHint></SearchHint>
            </FileRef>
            <FileRef>
                <HasRelativePath Value="false" />
                <RelativePath />
                <Name Value="B08.mp3" />
                <SearchHint><PathHint>
                    <RelativePathElement Dir="Studio" />
                    <RelativePathElement Dir="Acapellas" />
                </PathHint></SearchHint>
            </FileRef>
        </SampleRef></LiveSet></Ableton>"#;
        let refs = extract_sample_refs_from_xml(xml);
        assert_eq!(
            refs,
            vec![PathBuf::from("Studio/Acapellas/B08.mp3")],
            "the path hint must not be glued to itself"
        );
    }

    #[test]
    fn live9_relative_path_wins_over_hint() {
        let xml = r#"<Ableton><LiveSet><SampleRef>
            <FileRef>
                <HasRelativePath Value="true" />
                <RelativePath><RelativePathElement Dir="Samples" /></RelativePath>
                <Name Value="kick.wav" />
                <SearchHint><PathHint>
                    <RelativePathElement Dir="Elsewhere" />
                </PathHint></SearchHint>
            </FileRef>
        </SampleRef></LiveSet></Ableton>"#;
        let refs = extract_sample_refs_from_xml(xml);
        assert_eq!(refs, vec![PathBuf::from("Samples/kick.wav")]);
    }

    #[test]
    fn entity_in_sample_name_is_decoded() {
        let xml = r#"<Ableton><LiveSet><SampleRef>
            <FileRef>
                <HasRelativePath Value="true" />
                <RelativePath />
                <Name Value="A &amp; B &lt;mix&gt;.wav" />
            </FileRef>
        </SampleRef></LiveSet></Ableton>"#;
        let refs = extract_sample_refs_from_xml(xml);
        assert_eq!(refs, vec![PathBuf::from("A & B <mix>.wav")]);
    }
}

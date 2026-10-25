//! Tests for the shared marker name helper.
//!
//! A cue's text is written by a person and ends up inside a set, so
//! the helper that turns it into a marker name has to survive anything
//! a person might type.

use clavus_core::marker_name;

#[test]
fn a_short_cue_is_kept_whole() {
    assert_eq!(marker_name("drop the bass"), "● drop the bass");
}

#[test]
fn a_long_cue_is_cut_at_sixty_characters() {
    let text = "a".repeat(200);
    let name = marker_name(&text);
    // The bullet, a space, and 60 characters.
    assert_eq!(name.chars().count(), 62);
}

#[test]
fn a_multi_byte_character_is_never_cut_in_half() {
    // 59 ASCII characters and then an accent. Cutting at a byte
    // boundary here would panic.
    let mut text = "a".repeat(59);
    text.push('é');
    let name = marker_name(&text);
    assert_eq!(name.chars().count(), 62);
    assert!(name.ends_with('é'), "the accent should survive: {}", name);
}

#[test]
fn a_cue_of_only_accents_is_cut_at_sixty_characters() {
    let text = "é".repeat(100);
    let name = marker_name(&text);
    assert_eq!(name.matches('é').count(), 60);
}

#[test]
fn xml_characters_in_a_cue_are_escaped() {
    let name = marker_name(r#"say "hi" & <go>"#);
    assert_eq!(name, "● say &quot;hi&quot; &amp; &lt;go&gt;");
    assert!(!name.contains('"'), "a raw quote would break the attribute");
    assert!(!name.contains('<'), "a raw angle bracket would break the set");
}

#[test]
fn an_empty_cue_still_produces_a_name() {
    assert_eq!(marker_name(""), "● ");
}

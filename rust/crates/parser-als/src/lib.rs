//! Ableton `.als` parser.
//!
//! `.als` files are gzip-compressed XML. This module extracts:
//!   - Track names, colors, types (Audio/MIDI/Group/Return)
//!   - Arrangement markers
//!   - BPM and tempo envelope
//!   - Plugin/device chains
//!   - Freeze/mute/solo status
//!   - Sample references

pub mod als;
pub mod sample_ref;
pub mod marker;

pub use als::AlsParser;
pub use sample_ref::extract_sample_refs;
pub use marker::inject_markers;

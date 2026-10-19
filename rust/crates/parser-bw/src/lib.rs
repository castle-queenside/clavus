//! Bitwig `.bwproject` parser.
//!
//! `.bwproject` files are ZIP archives containing:
//!   - `project.xml` — main project XML
//!   - `samples/` — embedded audio samples (optional)
//!   - `presets/` — device presets (optional)
//!
//! Bitwig uses `<Locator>` for markers and `bars.beats.sixteenths` time notation.

pub mod bitwig;
pub mod locator;

pub use bitwig::BitwigParser;
pub use locator::inject_locators;

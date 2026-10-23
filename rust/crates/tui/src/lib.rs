//! Clavus TUI — terminal interface for managing DAW project snapshots and cues.

pub mod app;
pub mod keys;
pub mod mouse;
pub mod spark;
pub mod theme;
pub mod ui;

pub use app::App;
pub use keys::handle_key;

//! Clavus TUI theme — "Arch Dark" design system.
//!
//! Palette inspired by Arch Linux tooling precision and Omarchy minimalism.
//! Background  #0d1117  Surface  #161b22  Border  #30363d
//! Text        #e6edf3  Muted   #7d8590  Accent  #58a6ff
//! Gold        #d29922  Green   #3fb950  Red    #f85149

use ratatui::style::Color;

/// ─── Palette ─────────────────────────────────────────────────────────
pub const BG:        Color = Color::Rgb(13, 17, 23);   // deep ink
pub const SURFACE:   Color = Color::Rgb(22, 27, 34);   // card/panel bg
pub const BORDER:    Color = Color::Rgb(48, 54, 61);   // structural lines
pub const BORDER_DIM: Color = Color::Rgb(33, 38, 45);   // inactive borders

pub const TEXT:      Color = Color::Rgb(230, 237, 243); // primary text
pub const MUTED:     Color = Color::Rgb(125, 133, 144); // secondary text
pub const ACCENT:    Color = Color::Rgb(88, 166, 255);  // selection / links
pub const GOLD:      Color = Color::Rgb(210, 153, 34);  // markers / snapshots
pub const GREEN:     Color = Color::Rgb(63, 185, 80);   // success / resolved
pub const RED:       Color = Color::Rgb(248, 81, 73);   // error / danger
pub const CYAN:      Color = Color::Rgb(56, 216, 214);  // sync / connected

/// ─── Styles ────────────────────────────────────────────────────────
use ratatui::style::Style;

pub const S_BG:        Style = Style::new().bg(BG);
pub const S_SURFACE:   Style = Style::new().bg(SURFACE);
pub const S_ACCENT:    Style = Style::new().fg(ACCENT);
pub const S_GOLD:      Style = Style::new().fg(GOLD);
pub const S_GREEN:     Style = Style::new().fg(GREEN);
pub const S_RED:       Style = Style::new().fg(RED);
pub const S_CYAN:      Style = Style::new().fg(CYAN);
pub const S_MUTED:     Style = Style::new().fg(MUTED);
pub const S_TEXT:      Style = Style::new().fg(TEXT);

/// Accent text on accent bg (for selected items)
pub const S_SELECTED:  Style = Style::new().fg(BG).bg(ACCENT);
pub const S_SELECTED_TEXT: Style = Style::new().fg(TEXT);

/// ─── Cue status colors ─────────────────────────────────────────────
use clavus_core::types::CueStatus;

pub fn cue_status_color(s: &CueStatus) -> Color {
    match s {
        CueStatus::Pending   => ACCENT,
        CueStatus::Deferred  => MUTED,
        CueStatus::Resolved  => GREEN,
        CueStatus::Skipped   => RED,
        CueStatus::Archived  => GOLD,
    }
}

pub fn cue_status_icon(s: &CueStatus) -> &'static str {
    match s {
        CueStatus::Pending   => "○",
        CueStatus::Deferred  => "◔",
        CueStatus::Resolved  => "●",
        CueStatus::Skipped   => "✗",
        CueStatus::Archived  => "▣",
    }
}

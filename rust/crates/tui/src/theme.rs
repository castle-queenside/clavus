//! Clavus TUI theme — Omarchy-inspired design system.
//!
//! Palette drawn from Omarchy's warm-dark aesthetic and Cliamp's refinement.
//! Background   #0b0c0d  (warm black, not cold)
//! Panel layers #0e1012 → #121517 → #15181a
//! Lines       #4d585f   (visible, not harsh)
//! Accent      #74e08c   (soft green)
//! Warm        #e9b455   (amber, for gold/markers)
//! Text        #e9ece9   (warm off-white, not pure white)
//! Muted       #829297   (desaturated, but readable)
//!
//! Spacing is a 4pt grid: 4, 8, 12, 16, 20, 24...

use ratatui::style::{Color, Style};
use clavus_core::types::CueStatus;

// ─── Palette ─────────────────────────────────────────────────────────

/// Warm black background — the canvas everything sits on.
pub const BG:           Color = Color::Rgb(11, 12, 13);

/// Layered panel surfaces, like Omarchy's.
pub const PANEL_0:       Color = Color::Rgb(14, 16, 18);  // closest to bg
pub const PANEL_1:      Color = Color::Rgb(18, 21, 23);  // raised
pub const PANEL_2:      Color = Color::Rgb(21, 24, 26);  // elevated

/// Structural lines and borders.
/// Structural lines and borders.
///
/// These were so close to the panel behind them that the panels did
/// not read as panels at all, and the screen looked like one flat
/// sheet of text. They are quiet, but they are now actually there.
pub const LINE:          Color = Color::Rgb(77, 88, 95);  // a visible divider

/// The very dim variant, for hairlines inside a panel.
pub const LINE_LO:      Color = Color::Rgb(56, 66, 72);

/// Primary text — warm off-white, not harsh.
pub const TEXT:         Color = Color::Rgb(233, 236, 233);

/// Secondary text.
///
/// This used to be so dim that the keybinding hints in the footer
/// blended into the panel behind them. It is now bright enough to be
/// read without effort — 5.5:1 against the lightest panel, which is
/// comfortably past the 4.5:1 that WCAG asks of ordinary text — while
/// still sitting well below `TEXT` at 15:1, so the hierarchy between
/// primary and secondary text is intact.
pub const MUTED:        Color = Color::Rgb(130, 146, 151);

/// Interactive accent — soft green, Omarchy's signature.
pub const ACCENT:       Color = Color::Rgb(116, 224, 140);

/// The accent, used as a *background*.
///
/// A whole row filled with the full accent is a bright block that
/// moves as the cursor moves. It is hard to read against, and over a
/// remote session it is the thing that draws the eye. The selected
/// row is raised instead: a dark green that is clearly a different
/// surface from the panel around it, with the accent kept for the
/// text and the marker.
pub const ACCENT_DIM:   Color = Color::Rgb(24, 44, 31);

/// Warm amber — markers, snapshots, timestamps.
pub const WARM:         Color = Color::Rgb(233, 180, 85);

/// Gold for cue status / markers (alias of WARM for backward compat).
pub const GOLD:         Color = WARM;

/// Success / resolved cues.
pub const GREEN:        Color = Color::Rgb(116, 224, 140);

/// Error / danger / skipped.
pub const RED:          Color = Color::Rgb(248, 100, 97);

/// Sync / connected state.
pub const CYAN:         Color = Color::Rgb(92, 210, 229);

/// Muted purple — decorative accent.
pub const PURPLE:       Color = Color::Rgb(195, 154, 217);

// ─── Styles ─────────────────────────────────────────────────────────

pub const S_BG:          Style = Style::new().bg(BG);
pub const S_PANEL_0:     Style = Style::new().bg(PANEL_0);
pub const S_PANEL_1:     Style = Style::new().bg(PANEL_1);
pub const S_ACCENT:      Style = Style::new().fg(ACCENT);
pub const S_ACCENT_DIM:  Style = Style::new().bg(ACCENT_DIM);
pub const S_WARM:        Style = Style::new().fg(WARM);
pub const S_GOLD:        Style = Style::new().fg(GOLD);
pub const S_GREEN:       Style = Style::new().fg(GREEN);
pub const S_RED:         Style = Style::new().fg(RED);
pub const S_CYAN:        Style = Style::new().fg(CYAN);
pub const S_MUTED:       Style = Style::new().fg(MUTED);
pub const S_TEXT:        Style = Style::new().fg(TEXT);
pub const S_PURPLE:      Style = Style::new().fg(PURPLE);

// ─── The history sparkline ──────────────────────────────────────────

/// The colour of one bar of a project's history.
///
/// The bars are Clavus's green, from a whisper to the full accent. The
/// height already says *how much*; the colour only makes the shape of
/// a project's history easier to take in at a glance.
pub fn spark_color(height: u8) -> Color {
    match height {
        0 => LINE_LO,
        1 => Color::Rgb(40, 74, 52),
        2 => Color::Rgb(50, 92, 64),
        3 => Color::Rgb(60, 112, 78),
        4 => Color::Rgb(72, 136, 90),
        5 => Color::Rgb(86, 164, 104),
        6 => Color::Rgb(100, 192, 120),
        7 => Color::Rgb(110, 212, 132),
        _ => ACCENT,
    }
}

// ─── Cue status helpers ─────────────────────────────────────────────

pub fn cue_status_color(s: &CueStatus) -> Color {
    match s {
        CueStatus::Pending   => ACCENT,
        CueStatus::Deferred  => MUTED,
        CueStatus::Resolved  => GREEN,
        CueStatus::Skipped   => RED,
        CueStatus::Archived  => WARM,
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

// ─── Idle phrases ───────────────────────────────────────────────────
// Omarchy/Cliamp-style idle messages when lists are empty.

pub const IDLE_PHRASES: &[&str] = &[
    "No signals detected",
    "Nothing tracked yet",
    "Drop a project in the folder",
    "Clean slate",
    "Spinning up",
];

/// Returns a deterministic idle phrase based on index.
pub fn idle_phrase(index: usize) -> &'static str {
    IDLE_PHRASES[index % IDLE_PHRASES.len()]
}

// ─── Spacing grid (4pt) ─────────────────────────────────────────────
// 4, 8, 12, 16, 20, 24, 32...

pub const SP: u16 = 4;  // base unit

/// 4pt
pub fn sp1() -> u16 { SP }
/// 8pt
pub fn sp2() -> u16 { SP * 2 }
/// 12pt
pub fn sp3() -> u16 { SP * 3 }
/// 16pt
pub fn sp4() -> u16 { SP * 4 }
/// 20pt
pub fn sp5() -> u16 { SP * 5 }
/// 24pt
pub fn sp6() -> u16 { SP * 6 }

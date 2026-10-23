//! Mouse support.
//!
//! Clavus is keyboard-first, and the keyboard still does everything.
//! But a mouse is the fastest way to point at one row in a long list,
//! and the wheel is the fastest way to travel through a thousand sets.
//!
//! The mouse is told what is on the screen by the renderer, which
//! records a map of the rows it drew. The mouse handler looks a click
//! up in that map. The layout is computed in one place — the renderer
//! — so the map can never disagree with what is on screen.

use crate::app::App;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use std::time::{Duration, Instant};

/// Two clicks this close together, in this small a place, are a double
/// click.
pub const DOUBLE_CLICK: Duration = Duration::from_millis(400);

/// How far apart two clicks can be and still be a double click.
pub const DOUBLE_CLICK_SLOP: u16 = 1;

/// A click, remembered so that the next one can be compared with it.
#[derive(Debug, Clone, Copy)]
pub struct Click {
    /// Where the click was.
    pub x: u16,
    pub y: u16,
    /// When it happened.
    pub at: Instant,
}

/// A rectangle on the screen that means something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HitRegion {
    /// The left edge.
    pub x: u16,
    /// The top edge.
    pub y: u16,
    /// How wide it is.
    pub w: u16,
    /// How tall it is.
    pub h: u16,
    /// What it means.
    pub kind: HitKind,
}

impl HitRegion {
    /// Whether a point falls inside the region.
    pub fn contains(&self, x: u16, y: u16) -> bool {
        x >= self.x && x < self.x.saturating_add(self.w) && y >= self.y && y < self.y.saturating_add(self.h)
    }
}

/// What a part of the screen means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitKind {
    /// A row of the project list.
    ///
    /// The number is an index into the backing list of projects, not
    /// into the order on screen. The sort and the filter change the
    /// order, and the index has to survive both.
    ProjectRow(usize),
    /// A row of the cue list.
    CueRow(usize),
    /// A row of the snapshot list.
    SnapshotRow(usize),
    /// The scrollbar down the right-hand edge of a list.
    ///
    /// Clicking it moves the window to that part of the list.
    Scrollbar {
        /// How many rows there are in all.
        total: usize,
        /// How many rows are on screen at once.
        visible: usize,
        /// The top of the scrollbar on the screen.
        top: u16,
        /// How tall it is.
        height: u16,
    },
}

/// Handle a mouse event.
///
/// Returns `true` when the application should exit.
pub fn handle_mouse(event: &MouseEvent, app: &mut App) -> bool {
    match event.kind {
        MouseEventKind::ScrollUp => app.scroll_up(),
        MouseEventKind::ScrollDown => app.scroll_down(),
        MouseEventKind::Down(MouseButton::Left) => {
            let double = app.last_click.is_some_and(|c| {
                c.at.elapsed() < DOUBLE_CLICK
                    && c.x.abs_diff(event.column) <= DOUBLE_CLICK_SLOP
                    && c.y.abs_diff(event.row) <= DOUBLE_CLICK_SLOP
            });
            app.last_click = Some(Click {
                x: event.column,
                y: event.row,
                at: Instant::now(),
            });
            click(event.column, event.row, double, app);
        }
        // A drag is the wheel's cousin: hold the button and move, and
        // the row under the pointer is the row you are on. The mouse
        // reports a move for every cell it crosses, so the selection
        // follows it without any extra work.
        MouseEventKind::Drag(MouseButton::Left) => {
            drag(event.column, event.row, app);
        }
        _ => {}
    }
    false
}

/// A single click: pick the row under the pointer.
fn click(x: u16, y: u16, double: bool, app: &mut App) {
    let Some(region) = app.hit_at(x, y) else {
        return;
    };
    match region.kind {
        HitKind::ProjectRow(idx) => {
            app.selected_project = Some(idx);
            // A double click opens the set, the same as Enter. A single
            // click only picks the row: opening on the first click
            // would make it impossible to look at a row before
            // committing to it.
            if double {
                crate::keys::open_selected_project(app);
            }
        }
        HitKind::CueRow(idx) => {
            app.selected_cue = Some(idx);
        }
        HitKind::SnapshotRow(idx) => {
            app.selected_snapshot = Some(idx);
        }
        HitKind::Scrollbar {
            total,
            visible,
            top,
            height,
        } => {
            // The click's place down the scrollbar says which part of
            // the list to show. The clicked part is put in the middle
            // of the window, so the rows above it are still on screen.
            let position = (y.saturating_sub(top)) as f64 / height.max(1) as f64;
            let content = (position * total as f64).round() as usize;
            let offset = content.saturating_sub(visible / 2);
            app.scroll_to(offset);
        }
    }
}

/// A drag: the row under the pointer becomes the selected row.
fn drag(x: u16, y: u16, app: &mut App) {
    let Some(region) = app.hit_at(x, y) else {
        return;
    };
    match region.kind {
        HitKind::ProjectRow(idx) => app.selected_project = Some(idx),
        HitKind::CueRow(idx) => app.selected_cue = Some(idx),
        HitKind::SnapshotRow(idx) => app.selected_snapshot = Some(idx),
        HitKind::Scrollbar { .. } => {
            // A drag on the scrollbar keeps moving the window.
            click(x, y, false, app);
        }
    }
}

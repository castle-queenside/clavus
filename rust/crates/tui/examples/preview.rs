//! A picture of the project list, without a terminal.
//!
//! `cargo run -p clavus-tui --example preview` prints the list as the
//! TUI draws it, so the layout can be looked at while it is being
//! changed. The width can be given as an argument to see how the list
//! copes with a narrow window.
//!
//! This is a development aid. The TUI itself is in `src/ui.rs`.

use clavus_tui::app::{App, ProjectEntry, SortMode};
use clavus_tui::theme::{ACCENT, MUTED, TEXT};
use ratatui::{
    backend::TestBackend,
    style::Color,
    Terminal,
};
use std::path::PathBuf;

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn entry(name: &str, folder: &str, snaps: usize, open: usize, age: i64, bars: Vec<u8>) -> ProjectEntry {
    ProjectEntry {
        path: PathBuf::from(format!("/tmp/{}.als", name)),
        name: name.to_string(),
        folder: folder.to_string(),
        project_type: "Ableton".to_string(),
        head: Some("b503db30de0e94735d632586f46b93cd04095e4998a29b384cbf3e6165b8be7d".to_string()),
        snap_count: snaps,
        bpm: 128.0,
        track_count: 9,
        modified_secs: now() - age,
        open_cues: open,
        activity: bars,
    }
}

fn main() {
    let width: u16 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(100);
    let height: u16 = 16;

    let mut app = App::new(PathBuf::from("/tmp"));
    app.projects = vec![
        entry("Midnight Drive", "Clavus Demo", 12, 2, 40, vec![1, 2, 3, 5, 8, 6, 3, 2]),
        entry("Dub Session", "Clavus Demo", 3, 0, 7_200, vec![0, 0, 0, 0, 0, 0, 0, 4]),
        entry("Field Recording 03", "Archive/Audio", 41, 0, 900_000, vec![8, 7, 6, 5, 4, 3, 2, 1]),
        entry("Papercut", "Work In Progress", 7, 5, 86_400, vec![2, 2, 3, 3, 4, 4, 5, 6]),
        entry("Nightbus", "Archive/Old", 0, 0, 20_000_000, vec![]),
    ];
    app.selected_project = Some(3);
    app.sort = SortMode::Recent;
    app.set_filter(String::new());

    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|f| clavus_tui::ui::render(f, &mut app))
        .expect("draw");

    let buf = terminal.backend().buffer().clone();
    for y in 0..buf.area.height {
        let mut line = String::new();
        for x in 0..buf.area.width {
            let cell = &buf.content[(y * buf.area.width + x) as usize];
            line.push_str(cell.symbol());
        }
        println!("{}", line);
    }

    // And the colours, so a background that leaked through is visible.
    println!("\n── the colours on the first row ──");
    for x in 0..buf.area.width {
        let cell = &buf.content[(buf.area.width + x) as usize];
        if cell.fg != Color::Reset || cell.bg != Color::Reset {
            println!(
                "  x={:>3}  fg={:?}  bg={:?}  {:?}",
                x, cell.fg, cell.bg, cell.symbol()
            );
        }
    }
    let _ = (TEXT, MUTED, ACCENT);
}

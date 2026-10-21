//! Clavus binary entry point.
//!
//! The command implementations live in the `clavus-cli` crate so they can
//! be driven directly from tests. This binary only parses arguments and
//! dispatches.

use clavus_cli::{cmd_audit, cmd_collect, cmd_cue, cmd_cues, cmd_log, cmd_projects,
                 cmd_pull, cmd_push, cmd_relay, cmd_restore, cmd_snapshot, cmd_status,
                 clavus_root, projects_dir, Cli, Commands};
use clap::Parser;
use clavus_tui::app::App;
use clavus_tui::ui;
use ratatui::{backend::CrosstermBackend, Terminal};
use std::path::PathBuf;
use std::panic;

// ─── Entry point ─────────────────────────────────────────────────

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Quiet by default: the CLI prints its own human-readable messages.
    // Set RUST_LOG=clavus=debug (or =info) for verbose diagnostics.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("clavus=error".parse()?),
        )
        .with_writer(std::io::stderr)
        .init();

    let projects_dir = cli.dir.clone().unwrap_or_else(projects_dir);
    let clavus_root = cli
        .data
        .clone()
        .or_else(|| std::env::var_os("CLAVUS_DATA").map(PathBuf::from))
        .unwrap_or_else(clavus_root);

    // The TUI takes over the terminal, so it must not share the CLI's
    // stdout writer.
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();

    match cli.command.unwrap_or(Commands::Tui) {
        Commands::Tui => run_tui(projects_dir, clavus_root),
        Commands::Snapshot {
            project,
            message,
            collect,
        } => cmd_snapshot(&project, &message, collect, &clavus_root, &mut stdout, &mut stderr),
        Commands::Log { project } => cmd_log(&project, &clavus_root, &mut stdout, &mut stderr),
        Commands::Cue {
            project,
            text,
            position,
            track,
        } => cmd_cue(
            &project,
            &text,
            &position,
            track.as_deref(),
            &clavus_root,
            &mut stdout,
            &mut stderr,
        ),
        Commands::Cues { project, status } => {
            cmd_cues(&project, status.as_deref(), &clavus_root, &mut stdout, &mut stderr)
        }
        Commands::Relay {
            bind,
            bind_positional,
            state_dir,
        } => {
            let bind = bind_positional.unwrap_or(bind);
            cmd_relay(&bind, state_dir, &clavus_root, &mut stdout, &mut stderr)
        }
        Commands::Audit { project, strict } => {
            cmd_audit(&project, strict, &mut stdout, &mut stderr)
        }
        Commands::Collect { project } => {
            cmd_collect(&project, &clavus_root, &mut stdout, &mut stderr)
        }
        Commands::Push { relay, project } => {
            cmd_push(&relay, &project, &clavus_root, &mut stdout, &mut stderr)
        }
        Commands::Pull { relay, project } => {
            cmd_pull(&relay, &project, &clavus_root, &mut stdout, &mut stderr)
        }
        Commands::Projects { recursive } => {
            // `cli.dir` is None unless the user asked for a specific
            // directory; otherwise the standard locations are scanned.
            cmd_projects(recursive, cli.dir.as_deref(), &mut stdout, &mut stderr)
        }
        Commands::Status { project } => cmd_status(&project, &clavus_root, &mut stdout, &mut stderr),
        Commands::Restore { project, hash } => {
            cmd_restore(&project, &hash, &clavus_root, &mut stdout, &mut stderr)
        }
    }
}

// ─── TUI ─────────────────────────────────────────────────────────

fn run_tui(projects_dir: PathBuf, clavus_root: PathBuf) -> anyhow::Result<()> {
    let mut app = App::new(projects_dir);
    // Honour `--data` / `CLAVUS_DATA` for every read and write the TUI makes.
    app.clavus_root = clavus_root;
    app.refresh_projects()
        .map_err(|e| anyhow::anyhow!("Failed to scan projects: {}", e))?;

    // Restore terminal on panic
    panic::set_hook(Box::new(|_| {
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);
        let _ = crossterm::terminal::disable_raw_mode();
    }));

    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend)?;

    crossterm::execute!(std::io::stdout(), crossterm::terminal::EnterAlternateScreen)?;
    crossterm::terminal::enable_raw_mode()?;
    terminal.clear()?;

    loop {
        terminal.draw(|f| ui::render(f, &mut app))?;

        if let Ok(crossterm::event::Event::Key(key)) = crossterm::event::read() {
            if clavus_tui::handle_key(&key, &mut app) {
                break;
            }
        }

        // Pick up connection status changes from the background sync thread.
        app.poll_sync();
    }

    crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen)?;
    crossterm::terminal::disable_raw_mode()?;
    Ok(())
}

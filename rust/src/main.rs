//! Clavus binary entry point.
//!
//! The command implementations live in the `clavus-cli` crate so they can
//! be driven directly from tests. This binary only parses arguments and
//! dispatches.

use clavus_cli::{cmd_audit, cmd_collect, cmd_cue, cmd_cues, cmd_log, cmd_projects,
                 cmd_pull, cmd_push, cmd_relay, cmd_restore, cmd_snapshot, cmd_status,
                 clavus_root, projects_dir, resolve_project, Cli, Commands};
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

    match cli.command.unwrap_or(Commands::Tui { relay: None }) {
        Commands::Tui { relay } => run_tui(projects_dir, clavus_root, relay),
        Commands::Snapshot {
            project,
            message,
            collect,
        } => {
            let project = resolve_project(&project, cli.dir.as_deref());
            cmd_snapshot(&project, &message, collect, &clavus_root, &mut stdout, &mut stderr)
        }
        Commands::Log { project } => {
            let project = resolve_project(&project, cli.dir.as_deref());
            cmd_log(&project, &clavus_root, &mut stdout, &mut stderr)
        }
        Commands::Cue {
            project,
            text,
            position,
            track,
            author,
        } => {
            let project = resolve_project(&project, cli.dir.as_deref());
            cmd_cue(
                &project,
                &text,
                &position,
                track.as_deref(),
                author.as_deref(),
                &clavus_root,
                &mut stdout,
                &mut stderr,
            )
        }
        Commands::Cues { project, status } => {
            let project = resolve_project(&project, cli.dir.as_deref());
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
            let project = resolve_project(&project, cli.dir.as_deref());
            cmd_audit(&project, strict, &mut stdout, &mut stderr)
        }
        Commands::Collect { project } => {
            let project = resolve_project(&project, cli.dir.as_deref());
            cmd_collect(&project, &clavus_root, &mut stdout, &mut stderr)
        }
        Commands::Push { relay, project } => {
            let project = resolve_project(&project, cli.dir.as_deref());
            cmd_push(&relay, &project, &clavus_root, &mut stdout, &mut stderr)
        }
        Commands::Pull { relay, project } => {
            let project = resolve_project(&project, cli.dir.as_deref());
            cmd_pull(&relay, &project, &clavus_root, &mut stdout, &mut stderr)
        }
        Commands::Projects { recursive } => {
            // `cli.dir` is None unless the user asked for a specific
            // directory; otherwise the standard locations are scanned.
            cmd_projects(recursive, cli.dir.as_deref(), &mut stdout, &mut stderr)
        }
        Commands::Status { project } => {
            let project = resolve_project(&project, cli.dir.as_deref());
            cmd_status(&project, &clavus_root, &mut stdout, &mut stderr)
        }
        Commands::Restore { project, hash } => {
            let project = resolve_project(&project, cli.dir.as_deref());
            cmd_restore(&project, &hash, &clavus_root, &mut stdout, &mut stderr)
        }
    }
}

// ─── Resolving the project argument ─────────────────────────────

// ─── TUI ─────────────────────────────────────────────────────────

fn run_tui(
    projects_dir: PathBuf,
    clavus_root: PathBuf,
    relay: Option<String>,
) -> anyhow::Result<()> {
    let mut app = App::new(projects_dir);
    // Honour `--data` / `CLAVUS_DATA` for every read and write the TUI makes.
    app.clavus_root = clavus_root;
    if let Some(relay) = relay {
        app.set_relay_address(relay);
    }
    app.refresh_projects()
        .map_err(|e| anyhow::anyhow!("Failed to scan projects: {}", e))?;

    // The TUI draws into the terminal. When the input or the output is
    // not a terminal — a pipe, /dev/null, a job running in the
    // background — there is nothing to draw in. The check happens
    // before the alternate screen is entered, so a failure cannot
    // leave the user's terminal in it.
    {
        use std::io::IsTerminal;
        if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
            anyhow::bail!(
                "stdin and stdout must be a terminal — Clavus draws in the terminal, and there is none here"
            );
        }
    }

    // Restore terminal on panic
    panic::set_hook(Box::new(|_| {
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);
        let _ = crossterm::terminal::disable_raw_mode();
    }));

    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend)?;

    // A terminal with no size — a pipe, a closed pty, a window that was
    // never given a size — used to abort with an unexplained exit
    // code 101 and no message at all. The tool now says why it cannot
    // start.
    match terminal.size() {
        Ok(size) if size.width == 0 || size.height == 0 => {
            anyhow::bail!(
                "the terminal has no size ({}x{}) — Clavus needs a real terminal to draw in",
                size.width,
                size.height
            );
        }
        Ok(_) => {}
        Err(e) => {
            anyhow::bail!("cannot read the terminal size: {}", e);
        }
    }

    // Entering the alternate screen and switching to raw mode are two
    // steps that can fail on their own. When the second one fails,
    // the first has to be undone — otherwise the terminal is left in
    // the alternate screen with no way back but `reset`.
    crossterm::execute!(std::io::stdout(), crossterm::terminal::EnterAlternateScreen)?;
    if let Err(e) = crossterm::terminal::enable_raw_mode() {
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);
        anyhow::bail!("cannot switch the terminal to raw mode: {}", e);
    }
    if let Err(e) = terminal.clear() {
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);
        let _ = crossterm::terminal::disable_raw_mode();
        anyhow::bail!("cannot clear the terminal: {}", e);
    }

    loop {
        terminal.draw(|f| ui::render(f, &mut app))?;

        // The screen has to keep up with the background sync thread even
        // when the user is not pressing keys, so the event read has a
        // timeout instead of blocking forever.
        //
        // A read error means the terminal is gone (the input was closed,
        // for instance). The old loop ignored the error and spun
        // forever, so `clavus tui` hung with no way out.
        let key = match crossterm::event::poll(std::time::Duration::from_millis(150)) {
            Ok(true) => match crossterm::event::read() {
                Ok(crossterm::event::Event::Key(key)) => Some(key),
                Ok(_) => None,
                Err(e) => {
                    // Restore the terminal before leaving.
                    let _ = crossterm::execute!(
                        std::io::stdout(),
                        crossterm::terminal::LeaveAlternateScreen
                    );
                    let _ = crossterm::terminal::disable_raw_mode();
                    anyhow::bail!("the terminal is gone: {}", e);
                }
            },
            Ok(false) => None,
            Err(e) => {
                let _ = crossterm::execute!(
                    std::io::stdout(),
                    crossterm::terminal::LeaveAlternateScreen
                );
                let _ = crossterm::terminal::disable_raw_mode();
                anyhow::bail!("cannot read from the terminal: {}", e);
            }
        };

        if let Some(key) = key {
            if clavus_tui::handle_key(&key, &mut app) {
                break;
            }
        }

        // Pick up connection status changes from the background sync thread.
        app.poll_sync();

        // Keep the idle phrases and the spectrum moving.
        app.tick();
    }

    crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen)?;
    crossterm::terminal::disable_raw_mode()?;
    Ok(())
}

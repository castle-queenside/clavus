//! Clavus binary entry point.

use clap::{Parser, Subcommand};
use clavus_core::sample;
use clavus_tui::app::App;
use clavus_tui::ui;
use ratatui::{backend::CrosstermBackend, Terminal};
use std::path::{Path, PathBuf};
use std::panic;

/// Clavus — version control for DAW projects.
#[derive(Parser)]
#[command(name = "clavus")]
#[command(version = "0.1.0")]
struct Cli {
    /// Projects directory
    #[arg(short, long)]
    dir: Option<PathBuf>,

    /// Clavus data directory (default: ~/.clavus)
    #[arg(long)]
    data: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Launch the terminal UI
    Tui,
    /// Create a snapshot of a project
    Snapshot {
        project: PathBuf,
        #[arg(short, long)]
        message: Option<String>,
    },
    /// List snapshots for a project
    Log { project: PathBuf },
    /// Add a cue marker
    Cue {
        project: PathBuf,
        #[arg(short, long)]
        text: String,
        #[arg(short, long)]
        position: String,
        #[arg(long)]
        track: Option<String>,
    },
    /// List cues
    Cues {
        project: PathBuf,
        #[arg(short, long)]
        status: Option<String>,
    },
    /// Run the sync relay server
    Relay {
        #[arg(default_value = "127.0.0.1:4477")]
        bind: String,
    },
    /// Audit audio sample references
    Audit {
        project: PathBuf,
        /// Fail-fast on missing samples
        #[arg(short, long)]
        strict: bool,
    },
    /// Collect missing samples into project folder
    Collect { project: PathBuf },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env()
            .add_directive("clavus=info".parse()?))
        .init();

    let default_projects = dirs::audio_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")));

    let projects_dir = cli.dir.unwrap_or(default_projects);
    let clavus_root = cli.data.unwrap_or_else(|| {
        dirs::home_dir()
            .map(|h| h.join(".clavus"))
            .unwrap_or_else(|| PathBuf::from(".clavus"))
    });

    match cli.command.unwrap_or(Commands::Tui) {
        Commands::Tui => run_tui(projects_dir),
        Commands::Snapshot { project, message } => cmd_snapshot(&project, &message),
        Commands::Log { project } => cmd_log(&project),
        Commands::Cue { project, text, position, track } => {
            cmd_cue(&project, &text, &position, track.as_deref())
        }
        Commands::Cues { project, status } => cmd_cues(&project, status.as_deref()),
        Commands::Relay { bind } => cmd_relay(&bind),
        Commands::Audit { project, strict } => cmd_audit(&project, strict),
        Commands::Collect { project } => cmd_collect(&project, &clavus_root),
    }
}

// ─── TUI ────────────────────────────────────────────────────────────

fn run_tui(projects_dir: PathBuf) -> anyhow::Result<()> {
    let mut app = App::new(projects_dir);
    app.refresh_projects().map_err(anyhow::Error::msg)?;

    // Set up panic hook to restore terminal
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
            if handle_key(&key, &mut app) {
                break;
            }
        }
    }

    let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);
    let _ = crossterm::terminal::disable_raw_mode();
    Ok(())
}

fn handle_key(key: &crossterm::event::KeyEvent, app: &mut App) -> bool {
    use clavus_tui::app::Screen;
    use crossterm::event::{KeyCode, KeyModifiers};

    match (key.code, key.modifiers) {
        (KeyCode::Esc, _) | (KeyCode::Char('q'), KeyModifiers::NONE) => {
            if app.screen == Screen::ProjectList {
                return true;
            }
            app.screen = Screen::ProjectList;
            app.clear_messages();
        }
        (KeyCode::Char('c'), KeyModifiers::NONE) if app.screen == Screen::ProjectList => {
            if let Some(idx) = app.selected_project {
                if app.load_project(idx).is_ok() {
                    app.screen = Screen::ProjectDetail;
                }
            }
        }
        (KeyCode::Char('l'), KeyModifiers::NONE) if app.screen == Screen::ProjectDetail => {
            app.screen = Screen::CueList;
        }
        (KeyCode::Char('s'), KeyModifiers::NONE) if app.screen == Screen::ProjectDetail => {
            app.screen = Screen::Sync;
        }
        (KeyCode::Down, _) if app.screen == Screen::ProjectList => {
            if let Some(i) = app.selected_project {
                if i + 1 < app.projects.len() {
                    app.selected_project = Some(i + 1);
                }
            } else if !app.projects.is_empty() {
                app.selected_project = Some(0);
            }
        }
        (KeyCode::Up, _) if app.screen == Screen::ProjectList => {
            if let Some(i) = app.selected_project {
                if i > 0 {
                    app.selected_project = Some(i - 1);
                }
            }
        }
        _ => {}
    }
    false
}

// ─── Commands ───────────────────────────────────────────────────────

fn cmd_snapshot(project: &Path, message: &Option<String>) -> anyhow::Result<()> {
    let parser = if project.extension().and_then(|s| s.to_str()) == Some("bwproject") {
        clavus_parser_bw::BitwigParser::new(project)?.project
    } else {
        clavus_parser_als::AlsParser::new(project)?.project
    };

    let report = sample::audit_project(project, &parser)?;
    report.ensure_clean()
        .map_err(|e| anyhow::anyhow!("Audio audit failed: {}", e))?;

    println!("✓ All {} samples clean — ready to snapshot", report.collected.len());
    println!(
        "Project: {} · {} tracks · {:.0} BPM",
        parser.name, parser.track_count, parser.bpm
    );
    if let Some(m) = message {
        println!("Message: {}", m);
    }
    Ok(())
}

fn cmd_log(_project: &PathBuf) -> anyhow::Result<()> {
    println!("No snapshots yet. Run `clavus snapshot <path>` first.");
    Ok(())
}

fn cmd_cue(
    project: &Path,
    text: &str,
    position: &str,
    track: Option<&str>,
) -> anyhow::Result<()> {
    let root = dirs::home_dir()
        .map(|h| h.join(".clavus"))
        .unwrap_or_else(|| PathBuf::from(".clavus"));
    let name = project
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let store = clavus_core::CueStore::new(&root, &name)?;
    let cue = store.add(text, position, "cli", "", track.unwrap_or("Arrangement"))?;
    println!("✓ Cue added: {} ({})", cue.id, cue.position);
    Ok(())
}

fn cmd_cues(_project: &PathBuf, _status: Option<&str>) -> anyhow::Result<()> {
    println!("(cue list)");
    Ok(())
}

fn cmd_relay(bind: &str) -> anyhow::Result<()> {
    println!("Starting relay on {}...", bind);
    clavus_relay::run(bind)?;
    Ok(())
}

fn cmd_audit(project: &Path, strict: bool) -> anyhow::Result<()> {
    let parser = if project.extension().and_then(|s| s.to_str()) == Some("bwproject") {
        clavus_parser_bw::BitwigParser::new(project)?.project
    } else {
        clavus_parser_als::AlsParser::new(project)?.project
    };

    let report = sample::audit_project(project, &parser)?;

    if report.is_clean() {
        println!(
            "✓ All {} samples present and inside project folder",
            report.collected.len()
        );
    } else {
        if !report.missing.is_empty() {
            eprintln!("✗ {} samples missing:", report.missing.len());
            for s in &report.missing {
                eprintln!("  - {}", s.path.display());
            }
        }
        if !report.outside_project.is_empty() {
            eprintln!("✗ {} samples outside project folder:", report.outside_project.len());
            for s in &report.outside_project {
                eprintln!("  - {}", s.path.display());
            }
        }
        if strict {
            return Err(anyhow::anyhow!("Audio audit FAILED"));
        }
    }
    Ok(())
}

fn cmd_collect(project: &Path, _clavus_root: &Path) -> anyhow::Result<()> {
    let parser = if project.extension().and_then(|s| s.to_str()) == Some("bwproject") {
        clavus_parser_bw::BitwigParser::new(project)?.project
    } else {
        clavus_parser_als::AlsParser::new(project)?.project
    };

    let report = sample::audit_project(project, &parser)?;
    let missing = report.missing.len();

    if missing == 0 && report.outside_project.is_empty() {
        println!("✓ All {} samples already present", report.collected.len());
    } else {
        println!(
            "Need to collect: {} missing, {} outside project",
            missing,
            report.outside_project.len()
        );
    }
    Ok(())
}

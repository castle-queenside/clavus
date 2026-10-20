//! Clavus binary entry point.

use clap::{Parser, Subcommand};
use clavus_core::sample;
use clavus_sync::SyncClient;
use clavus_tui::app::App;
use clavus_tui::ui;
use ratatui::{backend::CrosstermBackend, Terminal};
use std::path::{Path, PathBuf};
use std::panic;

// ─── CLI definition ─────────────────────────────────────────────────

/// Clavus — version control for DAW projects (Ableton Live + Bitwig Studio).
#[derive(Parser)]
#[command(
    name = "clavus",
    version,
    about = "Version control for your DAW projects",
    long_about = None,
)]
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
    /// Audit audio samples and create a snapshot
    Snapshot {
        project: PathBuf,
        /// Snapshot message
        #[arg(short, long)]
        message: Option<String>,
    },
    /// List the snapshot chain
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
        /// Directory for persisted relay state
        #[arg(long)]
        state_dir: Option<PathBuf>,
    },
    /// Audit audio sample references
    Audit {
        project: PathBuf,
        /// Fail fast if any sample is missing
        #[arg(short, long)]
        strict: bool,
    },
    /// Collect missing audio samples into the project folder
    Collect { project: PathBuf },
    /// Push local snapshots to the relay server
    Push {
        /// Relay address (e.g. 192.168.1.10:4477)
        #[arg(short = 'r', long, default_value = "127.0.0.1:4477")]
        relay: String,
        /// Project path (required — needed to identify which snapshots to push)
        project: PathBuf,
    },
    /// Pull snapshots from the relay server
    Pull {
        /// Relay address
        #[arg(short = 'r', long, default_value = "127.0.0.1:4477")]
        relay: String,
        /// Project path
        project: PathBuf,
    },
    /// List all Ableton/Bitwig projects found in standard locations
    Projects {
        /// Search recursively into subdirectories
        #[arg(short, long)]
        recursive: bool,
    },
}

// ─── Entry point ─────────────────────────────────────────────────

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("clavus=info".parse()?),
        )
        .init();

    let projects_dir = cli.dir.unwrap_or_else(projects_dir);
    let clavus_root = cli.data.unwrap_or_else(clavus_root);

    match cli.command.unwrap_or(Commands::Tui) {
        Commands::Tui => run_tui(projects_dir),
        Commands::Snapshot { project, message } => cmd_snapshot(&project, &message),
        Commands::Log { project } => cmd_log(&project, &clavus_root),
        Commands::Cue { project, text, position, track } => {
            cmd_cue(&project, &text, &position, track.as_deref())
        }
        Commands::Cues { project, status } => cmd_cues(&project, status.as_deref()),
        Commands::Relay { bind, state_dir } => cmd_relay(&bind, state_dir),
        Commands::Audit { project, strict } => cmd_audit(&project, strict),
        Commands::Collect { project } => cmd_collect(&project, &clavus_root),
        Commands::Push { relay, project } => cmd_push(&relay, &project, &clavus_root),
        Commands::Pull { relay, project } => cmd_pull(&relay, &project, &clavus_root),
        Commands::Projects { recursive } => cmd_projects(recursive),
    }
}

// ─── TUI ─────────────────────────────────────────────────────────

fn run_tui(projects_dir: PathBuf) -> anyhow::Result<()> {
    let mut app = App::new(projects_dir);
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
            if handle_key(&key, &mut app) {
                break;
            }
        }
    }

    crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen)?;
    crossterm::terminal::disable_raw_mode()?;
    Ok(())
}

fn handle_key(key: &crossterm::event::KeyEvent, app: &mut App) -> bool {
    use clavus_tui::app::Screen;
    use crossterm::event::{KeyCode, KeyModifiers};

    match (key.code, key.modifiers) {
        // ── Quit / back ──────────────────────────────────
        (KeyCode::Esc, _) | (KeyCode::Char('q'), KeyModifiers::NONE) => {
            if app.screen == Screen::ProjectList {
                return true;
            }
            app.screen = Screen::ProjectList;
            app.clear_messages();
        }

        // ── Navigation ────────────────────────────────
        (KeyCode::Enter, _) | (KeyCode::Char('o'), KeyModifiers::NONE)
            if app.screen == Screen::ProjectList =>
        {
            if let Some(idx) = app.selected_project {
                if app.load_project(idx).is_ok() {
                    app.screen = Screen::ProjectDetail;
                }
            }
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

        // ── Project Detail ───────────────────────────
        (KeyCode::Char('c'), KeyModifiers::NONE) if app.screen == Screen::ProjectDetail => {
            app.set_info("snapshot: use CLI".into());
        }
        (KeyCode::Char('l'), KeyModifiers::NONE) if app.screen == Screen::ProjectDetail => {
            if app.load_cues().is_ok() {
                app.screen = Screen::CueList;
            }
        }
        (KeyCode::Char('n'), KeyModifiers::NONE) if app.screen == Screen::ProjectDetail => {
            if app.load_snapshots().is_ok() {
                app.screen = Screen::SnapshotList;
            }
        }
        (KeyCode::Char('s'), KeyModifiers::NONE)
            if app.screen == Screen::ProjectList || app.screen == Screen::ProjectDetail =>
        {
            app.screen = Screen::Sync;
        }

        // ── Cue List ───────────────────────────────
        (KeyCode::Down, _) if app.screen == Screen::CueList => {
            if let Some(i) = app.selected_cue {
                if i + 1 < app.cues.len() {
                    app.selected_cue = Some(i + 1);
                }
            } else if !app.cues.is_empty() {
                app.selected_cue = Some(0);
            }
        }
        (KeyCode::Up, _) if app.screen == Screen::CueList => {
            if let Some(i) = app.selected_cue {
                if i > 0 {
                    app.selected_cue = Some(i - 1);
                }
            }
        }
        (KeyCode::Char('a'), KeyModifiers::NONE) if app.screen == Screen::CueList => {
            app.screen = Screen::CueAdd;
        }
        (KeyCode::Char('r'), KeyModifiers::NONE) if app.screen == Screen::CueList => {
            app.set_info("cue resolve: use CLI".into());
        }
        (KeyCode::Char('f'), KeyModifiers::NONE) if app.screen == Screen::CueList => {
            use clavus_core::types::CueStatus;
            app.cue_filter = match app.cue_filter {
                None => Some(CueStatus::Pending),
                Some(CueStatus::Pending) => Some(CueStatus::Resolved),
                Some(CueStatus::Resolved) => Some(CueStatus::Skipped),
                _ => None,
            };
            let _ = app.load_cues();
        }

        // ── Cue Add ──────────────────────────────
        (KeyCode::Enter, _) if app.screen == Screen::CueAdd => {
            if !app.input_buf.is_empty() && app.project_detail.is_some() {
                // Save cue: position from input_buf, empty text, Arrangement track
                let root = clavus_root();
                let name = app.project_detail.as_ref()
                    .and_then(|p| p.path.file_stem())
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown");
                if let Ok(store) = clavus_core::CueStore::new(&root, name) {
                    if store.add(
                        "",
                        &app.input_buf,
                        "tui",
                        "",
                        "Arrangement",
                    ).is_ok() {
                        app.set_info("cue saved".into());
                        let _ = app.load_cues();
                    }
                }
            }
            app.input_buf.clear();
            app.screen = Screen::CueList;
        }
        (KeyCode::Char(c), KeyModifiers::NONE) if app.screen == Screen::CueAdd => {
            app.input_buf.push(c);
        }
        (KeyCode::Backspace, _) if app.screen == Screen::CueAdd => {
            app.input_buf.pop();
        }

        // ── Sync ──────────────────────────────────
        (KeyCode::Char('c'), KeyModifiers::NONE) if app.screen == Screen::Sync => {
            if app.sync_state.is_connected() {
                app.sync_disconnect();
            } else {
                app.sync_connect("127.0.0.1:4477");
            }
        }
        (KeyCode::Char('P'), KeyModifiers::NONE) if app.screen == Screen::Sync => {
            if app.sync_state.is_connected() {
                app.sync_state.send_cmd(clavus_tui::app::SyncCmd::Push);
                app.set_info("push sent".into());
            } else {
                app.set_info("press c to connect first".into());
            }
        }
        (KeyCode::Char('G'), KeyModifiers::NONE) if app.screen == Screen::Sync => {
            if app.sync_state.is_connected() {
                app.sync_state.send_cmd(clavus_tui::app::SyncCmd::Pull);
                app.set_info("pull sent".into());
            } else {
                app.set_info("press c to connect first".into());
            }
        }

        // ── Snapshot List ────────────────────────
        (KeyCode::Down, _) if app.screen == Screen::SnapshotList => {
            if let Some(i) = app.selected_snapshot {
                if i + 1 < app.snapshots.len() {
                    app.selected_snapshot = Some(i + 1);
                }
            } else if !app.snapshots.is_empty() {
                app.selected_snapshot = Some(0);
            }
        }
        (KeyCode::Up, _) if app.screen == Screen::SnapshotList => {
            if let Some(i) = app.selected_snapshot {
                if i > 0 {
                    app.selected_snapshot = Some(i - 1);
                }
            }
        }

        _ => {}
    }
    false
}

// ─── Commands ────────────────────────────────────────────────────

fn cmd_snapshot(project: &Path, message: &Option<String>) -> anyhow::Result<()> {
    use clavus_core::blob::BlobStore;
    use clavus_core::snapshot::SnapshotStore;

    let name = project
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    let parser = if project.extension().and_then(|s| s.to_str()) == Some("bwproject") {
        clavus_parser_bw::BitwigParser::new(project)?.project
    } else {
        clavus_parser_als::AlsParser::new(project)?.project
    };

    let report = sample::audit_project(project)?;
    if let Err(e) = report.ensure_clean() {
        eprintln!("✗ {}", e);
        for line in e.detail_lines() {
            eprintln!("{}", line);
        }
        eprintln!("\nRun `clavus collect {:?}` to copy missing/outside-project samples.", project);
        anyhow::bail!("sample audit failed");
    };

    let store = BlobStore::new(clavus_root().join("blobs"))
        .map_err(|e| anyhow::anyhow!("BlobStore: {}", e))?;
    let snap_store = SnapshotStore::new(store, &name);

    // Parent: current HEAD
    let parent = snap_store.head().map(|s| s.hash);

    // Build sample info from audit (collected + outside_project, all with hashes)
    let all_present: Vec<_> = report
        .collected
        .iter()
        .chain(report.outside_project.iter())
        .filter_map(|s| s.hash.as_ref().map(|h| (h.clone(), s.path.to_string_lossy().to_string())))
        .collect::<Vec<_>>();
    let sample_hashes: Vec<_> = all_present.iter().map(|(h, _)| h.clone()).collect();
    let sample_paths: std::collections::HashMap<_, _> = all_present.into_iter().collect();

    let msg = message
        .as_deref()
        .unwrap_or("auto snapshot");

    let snapshot = snap_store.save(
        project,
        msg,
        parent.as_deref(),
        parser.track_count,
        parser.bpm,
        sample_hashes,
        sample_paths,
    )?;

    println!(
        "✓ snapshot {}  ·  {}  ·  {} tracks  ·  {:.0} BPM",
        &snapshot.hash[..8],
        format_timestamp(snapshot.timestamp),
        snapshot.track_count,
        snapshot.bpm,
    );
    Ok(())
}

fn cmd_log(project: &Path, clavus_root: &Path) -> anyhow::Result<()> {
    use clavus_core::blob::BlobStore;
    use clavus_core::snapshot::SnapshotStore;

    let name = project
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    let store = BlobStore::new(clavus_root.join("blobs"))
        .map_err(|e| anyhow::anyhow!("BlobStore: {}", e))?;
    let snap_store = SnapshotStore::new(store, &name);

    let head = match snap_store.head() {
        Some(h) => h,
        None => {
            println!("No snapshots yet — run `clavus snapshot <path>` first.");
            return Ok(());
        }
    };

    let chain = snap_store.walk_chain(Some(&head.hash));
    println!("{} snapshot(s) in chain\n", chain.len());

    for s in &chain {
        let parent = s.parent.as_ref().map(|p| format!(" <- {}", &p[..8])).unwrap_or_default();
        let ts = format_timestamp(s.timestamp);
        println!(
            "  {}  {}  {}{}  [{} tracks · {:.0} BPM]",
            &s.hash[..8],
            ts,
            s.message.replace('\n', " "),
            parent,
            s.track_count,
            s.bpm,
        );
    }
    Ok(())
}

fn cmd_cue(
    project: &Path,
    text: &str,
    position: &str,
    track: Option<&str>,
) -> anyhow::Result<()> {
    let root = clavus_root();
    let name = project
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let store = clavus_core::CueStore::new(&root, &name)?;
    let cue = store.add(text, position, "cli", "", track.unwrap_or("Arrangement"))?;
    println!("✓ cue {}  @ {}  ({})", &cue.id[..8], cue.position, cue.status.as_str());
    Ok(())
}

fn cmd_cues(project: &Path, status: Option<&str>) -> anyhow::Result<()> {
    let root = clavus_root();
    let name = project
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let store = clavus_core::CueStore::new(&root, &name)?;
    let cues = store.list(status)?;

    if cues.is_empty() {
        println!("No cues.");
    } else {
        for c in &cues {
            let text = if c.text.len() > 22 {
                format!("{}…", &c.text[..22])
            } else {
                c.text.clone()
            };
            println!(
                "  {}  [{:9}]  {:22}  @ {:>8}  {}",
                &c.id[..8],
                c.status.as_str(),
                text,
                c.position,
                c.author
            );
        }
        println!("\n{} cue(s)", cues.len());
    }
    Ok(())
}

fn cmd_relay(bind: &str, state_dir: Option<PathBuf>) -> anyhow::Result<()> {
    let dir = state_dir.unwrap_or_else(clavus_root).join("relay-state");
    println!("Listening on {}…", bind);
    println!("State: {}", dir.display());
    clavus_relay::run(bind, dir)?;
    Ok(())
}

fn cmd_audit(project: &Path, strict: bool) -> anyhow::Result<()> {
    let report = sample::audit_project(project)?;

    if report.is_clean() {
        println!("✓ {} samples present", report.collected.len());
    } else {
        if !report.missing.is_empty() {
            eprintln!("✗ {} missing:", report.missing.len());
            for s in &report.missing {
                eprintln!("  {}", s.path.display());
            }
        }
        if !report.outside_project.is_empty() {
            eprintln!("✗ {} outside project:", report.outside_project.len());
            for s in &report.outside_project {
                eprintln!("  {}", s.path.display());
            }
        }
        if strict {
            return Err(anyhow::anyhow!("Audio audit failed"));
        }
    }
    Ok(())
}

fn cmd_collect(project: &Path, _clavus_root: &Path) -> anyhow::Result<()> {
    let report = sample::audit_project(project)?;

    if report.is_clean() {
        println!("✓ {} samples already present, all inside project folder", report.collected.len());
        return Ok(());
    }

    println!(
        "  {} missing  ·  {} outside project",
        report.missing.len(),
        report.outside_project.len()
    );

    // Copy outside-project samples into project/Samples/ and rewrite paths
    let collect = sample::collect_samples(project, &report)?;

    if collect.copied.is_empty() && !collect.failed.is_empty() {
        println!("\n✗ Failed to collect any samples:");
        for (path, err) in &collect.failed {
            println!("  {} — {}", path.display(), err);
        }
    } else if !collect.copied.is_empty() {
        println!("\n✓ Collected {} sample(s) into project/Samples/", collect.copied.len());
        println!("  Paths in .als rewritten — restart Ableton to reload");
        if !collect.failed.is_empty() {
            println!("  ⚠ {} sample(s) could not be copied:", collect.failed.len());
            for (path, err) in &collect.failed {
                println!("    {} — {}", path.display(), err);
            }
        }
    }

    Ok(())
}

// ─── Sync commands ────────────────────────────────────────────────────

fn cmd_push(relay: &str, project: &Path, clavus_root: &Path) -> anyhow::Result<()> {
    use clavus_core::blob::BlobStore;
    use clavus_core::snapshot::SnapshotStore;
    use clavus_sync::SyncMsg;

    let name = project
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    // Load local snapshot store
    let store = BlobStore::new(clavus_root.join("blobs"))
        .map_err(|e| anyhow::anyhow!("BlobStore: {}", e))?;
    let snap_store = SnapshotStore::new(store, &name);

    // Collect all local snapshots in the chain
    let mut snapshots = Vec::new();
    let mut current = snap_store.head();
    let mut seen = std::collections::HashSet::new();
    while let Some(head) = current {
        if seen.contains(&head.hash) {
            break; // prevent loop
        }
        seen.insert(head.hash.clone());
        snapshots.push(head.clone());
        current = head.parent.as_ref().and_then(|p| snap_store.load(p));
    }
    snapshots.reverse(); // root → latest

    if snapshots.is_empty() {
        println!("No local snapshots to push for '{}'", name);
        return Ok(());
    }

    println!("Connecting to {}...", relay);
    let client = SyncClient::connect(relay)
        .map_err(|e| anyhow::anyhow!("Failed to connect to {}: {}", relay, e))?;

    // Say hello with empty known_heads so relay sends us everything
    client.hello(&name, vec![])
        .map_err(|e| anyhow::anyhow!("Hello: {}", e))?;

    // Receive relay's state
    let resp = client.recv_timeout(std::time::Duration::from_secs(5));
    match resp {
        Some(SyncMsg::Push { snapshots: _, cues: _ }) => {
            println!("  relay has {} snapshot(s), {} cue(s)", 0, 0);
        }
        Some(_) | None => {
            // Relay sent something else or nothing — that's fine
        }
    }

    // Push our local snapshots
    let push_snaps: Vec<_> = snapshots.into_iter().map(|s| {
        // Reconstruct a minimal Snapshot from what's in the store
        clavus_core::types::Snapshot {
            hash: s.hash.clone(),
            parent: s.parent.clone(),
            timestamp: s.timestamp,
            message: s.message.clone(),
            track_count: s.track_count,
            bpm: s.bpm,
            tags: s.tags.clone(),
            notes: s.notes.clone(),
            sample_hashes: s.sample_hashes.clone(),
            sample_paths: s.sample_paths.clone(),
            project_path: s.project_path.clone(),
        }
    }).collect();

    println!("Pushing {} snapshot(s)...", push_snaps.len());
    client.pull_resp(push_snaps, vec![])
        .map_err(|e| anyhow::anyhow!("Push: {}", e))?;

    // Wait for Synced or Conflict
    let timeout = std::time::Duration::from_secs(10);
    loop {
        match client.recv_timeout(timeout) {
            Some(SyncMsg::Synced) => {
                println!("✓ All snapshots pushed successfully");
                break;
            }
            Some(SyncMsg::Conflict { project, relay_head, push_head }) => {
                println!("⚠ Conflict detected on '{}': relay head {} vs our latest {}",
                    project, &relay_head[..8], &push_head[..8]);
                println!("  Run `clavus pull --relay {}` first to merge.", relay);
                break;
            }
            Some(msg) => {
                println!("  relay: {:?}", msg);
            }
            None => {
                println!("⚠ Relay timed out waiting for acknowledgement");
                break;
            }
        }
    }

    Ok(())
}

fn cmd_pull(relay: &str, project: &Path, clavus_root: &Path) -> anyhow::Result<()> {
    use clavus_core::blob::BlobStore;
    use clavus_core::snapshot::SnapshotStore;
    use clavus_sync::SyncMsg;

    let name = project
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    // Load local snapshot store
    let store = BlobStore::new(clavus_root.join("blobs"))
        .map_err(|e| anyhow::anyhow!("BlobStore: {}", e))?;
    let snap_store = SnapshotStore::new(store, &name);

    // Get local HEAD(s) so we tell relay what we already have
    let known_heads: Vec<String> = snap_store
        .head()
        .map(|h| h.hash)
        .into_iter()
        .collect();

    println!("Connecting to {}...", relay);
    let client = SyncClient::connect(relay)
        .map_err(|e| anyhow::anyhow!("Failed to connect to {}: {}", relay, e))?;

    // Announce what we know
    client.hello(&name, known_heads)
        .map_err(|e| anyhow::anyhow!("Hello: {}", e))?;

    // Receive relay's response (Push with missing snapshots)
    let timeout = std::time::Duration::from_secs(10);
    let mut pulled_count = 0;
    loop {
        match client.recv_timeout(timeout) {
            Some(SyncMsg::Push { snapshots, cues: _ }) => {
                for snap in &snapshots {
                    if !snap_store.has(&snap.hash) {
                        snap_store.save_raw(snap.clone())?;
                        println!("  ↳ {}  {}", &snap.hash[..8], snap.message);
                        pulled_count += 1;
                    }
                }
                if snapshots.is_empty() {
                    println!("  Relay has no new snapshots");
                }
                client.synced().ok();
                break;
            }
            Some(SyncMsg::Conflict { project, relay_head, push_head }) => {
                println!("⚠ Conflict detected on '{}'", project);
                println!("  relay HEAD: {}  your HEAD: {}",
                    &relay_head[..8], &push_head[..8]);
                break;
            }
            Some(msg) => {
                println!("  relay: {:?}", msg);
            }
            None => {
                println!("⚠ Relay timed out — no response");
                break;
            }
        }
    }

    if pulled_count > 0 {
        println!("✓ Pulled {} new snapshot(s)", pulled_count);
    } else {
        println!("✓ Already up to date");
    }

    Ok(())
}

// ─── Helpers ────────────────────────────────────────────────────

/// Standard Ableton project locations to scan.
fn standard_albeton_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(music) = dirs::audio_dir() {
        dirs.push(music);
    }
    if let Some(home) = dirs::home_dir() {
        let documents = home.join("Documents");
        if documents.exists() {
            dirs.push(documents);
        }
    }
    dirs
}

fn cmd_projects(recursive: bool) -> anyhow::Result<()> {
    let dirs = standard_albeton_dirs();
    if dirs.is_empty() {
        println!("No standard locations found. Pass --dir explicitly.");
        return Ok(());
    }

    let mut projects: Vec<(String, PathBuf)> = Vec::new();

    for base in dirs {
        find_projects_in(&base, recursive, &mut projects);
    }

    if projects.is_empty() {
        println!("No Ableton/Bitwig projects found.");
        println!("Hint: pass --dir to scan a specific folder, or use clavus tui");
        return Ok(());
    }

    println!("Found {} project(s):\n", projects.len());
    for (name, path) in &projects {
        let modified = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .map(format_modified_time)
            .unwrap_or_default();

        println!("  {}  {}", name, modified);
        println!("  {}", path.display());
        println!();
    }
    Ok(())
}

fn format_modified_time(t: std::time::SystemTime) -> String {
    let dur = t.duration_since(std::time::UNIX_EPOCH).unwrap();
    // Simple UTC format: YYYY-MM-DD HH:MM
    let secs = dur.as_secs();
    let days = secs / 86400;
    let rem = secs % 86400;
    let hours = rem / 3600;
    let mins = (rem % 3600) / 60;
    // Use a base date for Unix epoch and add days
    // 1970-01-01 + days days
    let year_base = 1970;
    let mut year = year_base;
    let mut days_left = days as i64;
    while days_left > 0 {
        let days_in_year = if is_leap(year) { 366 } else { 365 };
        if days_left >= days_in_year {
            days_left -= days_in_year;
            year += 1;
        } else {
            break;
        }
    }
    let yday = days_left as usize;
    let month_day = month_and_day(year, yday);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        year, month_day.0, month_day.1, hours, mins
    )
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

fn month_and_day(year: i64, yday: usize) -> (u8, u8) {
    // Days per month (non-leap)
    let days_in_month: [u16; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let days_in_month = if is_leap(year) {
        let mut d = days_in_month;
        d[1] = 29;
        d
    } else {
        days_in_month
    };
    let mut remaining = yday as u16;
    for (i, &dm) in days_in_month.iter().enumerate() {
        if remaining < dm {
            return ((i + 1) as u8, (remaining + 1) as u8);
        }
        remaining -= dm;
    }
    (12, 31)
}

fn find_projects_in(dir: &Path, recursive: bool, out: &mut Vec<(String, PathBuf)>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if recursive {
                find_projects_in(&path, recursive, out);
            }
            continue;
        }

        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        if ext == "als" || ext == "bwproject" {
            if let Some(name) = path.file_stem().map(|s| s.to_string_lossy().to_string()) {
                out.push((name, path));
            }
        }
    }
}

/// Default projects directory: audio dir, or home.
fn projects_dir() -> PathBuf {
    dirs::audio_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Default Clavus data directory: ~/.clavus
fn clavus_root() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".clavus"))
        .unwrap_or_else(|| PathBuf::from(".clavus"))
}

/// Format a Unix timestamp as YYYY-MM-DD HH:MM (no external deps).
fn format_timestamp(secs: f64) -> String {
    let secs = secs as i64;
    let days = secs / 86400;
    let rem = secs % 86400;
    let hour = rem / 3600;
    let min = rem % 3600 / 60;

    // Zeller-like month calculation from days since Unix epoch
    // 1970-01-01 was a Thursday (weekday=4)
    let mut year = 1970;
    let mut ydays = days;
    loop {
        let ylen = if (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0) {
            366
        } else {
            365
        };
        if ydays < ylen {
            break;
        }
        ydays -= ylen;
        year += 1;
    }

    // Days per month (non-leap), we'll adjust below
    static MPD: [i64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    #[allow(unused_mut)]
    let mut month = 11;
    let mut mday = ydays;
    if (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0) {
        // leap year — Feb has 29 days
        let days = [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        for (i, &d) in days.iter().enumerate() {
            if mday < d {
                month = i;
                break;
            }
            mday -= d;
        }
    } else {
        for (i, &d) in MPD.iter().enumerate() {
            if mday < d {
                month = i;
                break;
            }
            mday -= d;
        }
    }

    format!(
        "{}-{:02}-{:02} {:02}:{:02}",
        year,
        month + 1,
        mday + 1,
        hour,
        min
    )
}

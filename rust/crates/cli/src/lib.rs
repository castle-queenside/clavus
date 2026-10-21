//! Clavus command-line interface.
//!
//! The command implementations live here rather than in the binary so they
//! can be driven directly from tests, with output written to injectable
//! streams.

use clap::{Parser, Subcommand};
use clavus_core::sample;
use clavus_sync::SyncClient;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Write a line to a stream, ignoring I/O errors.
///
/// A closed or broken stdout must not abort a command that has already
/// done its real work, so write failures are deliberately swallowed.
macro_rules! outln {
    ($out:expr, $($arg:tt)*) => {{
        let _ = writeln!($out, $($arg)*);
    }};
}

/// Write a line to the error stream, ignoring I/O errors.
macro_rules! errln {
    ($err:expr, $($arg:tt)*) => {{
        let _ = writeln!($err, $($arg)*);
    }};
}

// ─── CLI definition ─────────────────────────────────────────────────

/// Clavus — version control for DAW projects (Ableton Live + Bitwig Studio).
#[derive(Parser, Debug)]
#[command(
    name = "clavus",
    version,
    about = "Version control for your DAW projects",
    long_about = None,
)]
pub struct Cli {
    /// Projects directory
    #[arg(short, long)]
    pub dir: Option<PathBuf>,

    /// Clavus data directory (default: ~/.clavus)
    #[arg(long)]
    pub data: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Launch the terminal UI
    Tui,
    /// Audit audio samples and create a snapshot
    Snapshot {
        project: PathBuf,
        /// Snapshot message
        #[arg(short, long)]
        message: Option<String>,
        /// Auto-run `collect` before snapshot if any samples are outside project
        #[arg(short = 'c', long)]
        collect: bool,
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
        /// Bind address, e.g. 0.0.0.0:4477
        #[arg(short, long, default_value = "127.0.0.1:4477")]
        bind: String,
        /// Bind address (positional form, overrides --bind)
        bind_positional: Option<String>,
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
    /// Show project health at a glance: snapshots, samples, cues, relay sync
    Status {
        project: PathBuf,
    },
    /// Restore a project to a previous snapshot
    Restore {
        project: PathBuf,
        /// Snapshot hash (or part of one — will be matched against local snapshots)
        #[arg(default_value = "HEAD")]
        hash: String,
    },
}

pub fn cmd_snapshot(
    project: &Path,
    message: &Option<String>,
    collect: bool,
    clavus_root: &Path,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> anyhow::Result<()> {
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
        errln!(err, "✗ {}", e);
        for line in e.detail_lines() {
            errln!(err, "{}", line);
        }
        if collect {
            errln!(err, "\n→ Running `clavus collect` to copy samples into project folder...");
            sample::collect_samples(project, &report)?;
            // Re-audit after collecting
            let report = sample::audit_project(project)?;
            if let Err(e) = report.ensure_clean() {
                errln!(err, "✗ Even after collecting, audit still failed:");
                for line in e.detail_lines() {
                    errln!(err, "{}", line);
                }
                anyhow::bail!("sample audit failed even after collecting");
            }
            outln!(out, "✓ Collect complete — proceeding with snapshot");
        } else {
            errln!(err, "\nRun `clavus collect {:?}` to copy missing/outside-project samples.", project);
            errln!(err, "Or: clavus snapshot {:?} --collect  (auto-collects before snapshot)", project);
            anyhow::bail!("sample audit failed");
        }
    }

    let store = BlobStore::new(clavus_root.join("blobs"))
        .map_err(|e| anyhow::anyhow!("BlobStore: {}", e))?;
    let snap_store = SnapshotStore::new(store, &name);

    // Parent: current HEAD
    let parent = snap_store.head().map(|s| s.hash);
    // Remember where HEAD points now, so an unchanged file can be reported
    // honestly rather than claiming a new snapshot was created.
    let previous_head = parent.clone();

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

    if previous_head.as_deref() == Some(snapshot.hash.as_str()) {
        // The project file is byte-identical to the current snapshot, so
        // nothing new was stored and the message was not recorded.
        outln!(out,
            "= snapshot {}  ·  unchanged — no new snapshot was created",
            clavus_core::short(&snapshot.hash, 8),
        );
        outln!(out, "  (the message \"{}\" was not recorded — change the project first)", msg);
        return Ok(());
    }

    outln!(out, 
        "✓ snapshot {}  ·  {}  ·  {} tracks  ·  {:.0} BPM",
        clavus_core::short(&snapshot.hash, 8),
        format_timestamp(snapshot.timestamp),
        snapshot.track_count,
        snapshot.bpm,
    );
    Ok(())
}

pub fn cmd_log(project: &Path, clavus_root: &Path,
    out: &mut dyn Write,
    _err: &mut dyn Write,
) -> anyhow::Result<()> {
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
            outln!(out, "No snapshots yet — run `clavus snapshot <path>` first.");
            return Ok(());
        }
    };

    let chain = snap_store.walk_chain(Some(&head.hash));
    outln!(out, "{} snapshot(s) in chain\n", chain.len());

    for s in &chain {
        let parent = s
            .parent
            .as_ref()
            .map(|p| format!(" <- {}", clavus_core::short(p, 8)))
            .unwrap_or_default();
        let ts = format_timestamp(s.timestamp);
        outln!(out, 
            "  {}  {}  {}{}  [{} tracks · {:.0} BPM]",
            clavus_core::short(&s.hash, 8),
            ts,
            s.message.replace('\n', " "),
            parent,
            s.track_count,
            s.bpm,
        );
    }
    Ok(())
}

pub fn cmd_cue(
    project: &Path,
    text: &str,
    position: &str,
    track: Option<&str>,
    clavus_root: &Path,
    out: &mut dyn Write,
    _err: &mut dyn Write,
) -> anyhow::Result<()> {
    let root = clavus_root;
    let name = project
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let store = clavus_core::CueStore::new(root, &name)?;
    let cue = store.add(text, position, "cli", "", track.unwrap_or("Arrangement"))?;
    outln!(out, "✓ cue {}  @ {}  ({})",
        &cue.id[..8.min(cue.id.len())],
        cue.position,
        cue.status.as_str());
    outln!(out, "  {}", truncate(&cue.text, 60));
    Ok(())
}

pub fn cmd_cues(project: &Path, status: Option<&str>, clavus_root: &Path,
    out: &mut dyn Write,
    _err: &mut dyn Write,
) -> anyhow::Result<()> {
    let root = clavus_root;
    let name = project
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let store = clavus_core::CueStore::new(root, &name)?;
    let cues = store.list(status)?;

    if cues.is_empty() {
        outln!(out, "No cues.");
    } else {
        for c in &cues {
            let text = truncate(&c.text, 22);
            outln!(out,
                "  {}  [{:9}]  {:22}  @ {:>8}  {}",
                truncate(&c.id, 8),
                c.status.as_str(),
                text,
                c.position,
                c.author
            );
        }
        outln!(out, "\n{}", plural(cues.len(), "cue", "cues"));
    }
    Ok(())
}

pub fn cmd_relay(bind: &str, state_dir: Option<PathBuf>, clavus_root: &Path,
    out: &mut dyn Write,
    _err: &mut dyn Write,
) -> anyhow::Result<()> {
    let dir = state_dir
        .unwrap_or_else(|| clavus_root.join("relay-state"));
    outln!(out, "Listening on {}…", bind);
    outln!(out, "State: {}", dir.display());
    clavus_relay::run(bind, dir)?;
    Ok(())
}

/// Truncate `s` to at most `max` characters, appending an ellipsis.
///
/// Works on characters rather than bytes, so a multi-byte character at the
/// cut point is never split — slicing bytes would panic on a boundary.
fn truncate(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

/// Format a count with a correctly pluralised noun.
fn plural(count: usize, singular: &str, plural: &str) -> String {
    if count == 1 {
        format!("{} {}", count, singular)
    } else {
        format!("{} {}", count, plural)
    }
}

pub fn cmd_audit(project: &Path, strict: bool,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> anyhow::Result<()> {
    let report = sample::audit_project(project)?;

    if report.is_clean() {
        outln!(out, "✓ {} present", plural(report.collected.len(), "sample", "samples"));
    } else {
        if !report.missing.is_empty() {
            errln!(err, "✗ {} missing:", report.missing.len());
            for s in &report.missing {
                errln!(err, "  {}", s.path.display());
            }
        }
        if !report.outside_project.is_empty() {
            errln!(err, "✗ {} outside project:", report.outside_project.len());
            for s in &report.outside_project {
                errln!(err, "  {}", s.path.display());
            }
        }
        if strict {
            return Err(anyhow::anyhow!("Audio audit failed"));
        }
    }
    Ok(())
}

pub fn cmd_collect(project: &Path, _clavus_root: &Path,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> anyhow::Result<()> {
    let report = sample::audit_project(project)?;

    if report.is_clean() {
        outln!(out, "✓ {} already present, all inside project folder",
            plural(report.collected.len(), "sample", "samples"));
        return Ok(());
    }

    outln!(out, 
        "  {} missing  ·  {} outside project",
        report.missing.len(),
        report.outside_project.len()
    );

    // Copy outside-project samples into project/Samples/ and rewrite paths
    let collect = sample::collect_samples(project, &report)?;

    if !collect.copied.is_empty() {
        outln!(out, "\n✓ Collected {} into project/Samples/",
            plural(collect.copied.len(), "sample", "samples"));
        outln!(out, "  Paths in .als rewritten — restart Ableton to reload");
    }

    if !collect.failed.is_empty() {
        errln!(err, "\n✗ Could not collect {}:",
            plural(collect.failed.len(), "sample", "samples"));
        for (path, reason) in &collect.failed {
            errln!(err, "  {} — {}", path.display(), reason);
        }
        errln!(err, "\nA missing sample cannot be collected — find the file and put it in the project, or remove the clip that references it.");
        anyhow::bail!("{} sample(s) could not be collected", collect.failed.len());
    }

    if collect.copied.is_empty() {
        outln!(out, "\nNothing to collect.");
    }

    Ok(())
}

pub fn cmd_push(relay: &str, project: &Path, clavus_root: &Path,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> anyhow::Result<()> {
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
        outln!(out, "No local snapshots to push for '{}'", name);
        return Ok(());
    }

    outln!(out, "Connecting to {}...", relay);
    let client = SyncClient::connect(relay)
        .map_err(|e| anyhow::anyhow!("Failed to connect to {}: {}", relay, e))?;

    // Say hello with empty known_heads so relay sends us everything
    client.hello(&name, vec![])
        .map_err(|e| anyhow::anyhow!("Hello: {}", e))?;

    // Receive relay's state
    let resp = client.recv_timeout(std::time::Duration::from_secs(5));
    match resp {
        Some(SyncMsg::Push { snapshots, cues }) => {
            outln!(out, "  relay has {} snapshot(s), {} cue(s)", snapshots.len(), cues.len());
        }
        Some(_) | None => {
            // Relay sent something else or nothing — that's fine
        }
    }

    // Push our local snapshots and cues.
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

    // The project's cues travel with the snapshots — a collaborator's
    // notes are the point of the cue system.
    let push_cues = match clavus_core::CueStore::new(clavus_root, &name) {
        Ok(store) => store.list(None).unwrap_or_default(),
        Err(e) => {
            errln!(err, "  ⚠ Could not open the cue store: {}", e);
            Vec::new()
        }
    };

    let push_count = push_snaps.len();
    let push_cue_count = push_cues.len();
    outln!(out, "Pushing {} snapshot(s), {} cue(s)...", push_count, push_cue_count);
    client
        .push(push_snaps, push_cues)
        .map_err(|e| anyhow::anyhow!("Push: {}", e))?;

    // Wait for Synced or Conflict
    let timeout = std::time::Duration::from_secs(10);
    loop {
        match client.recv_timeout(timeout) {
            Some(SyncMsg::Synced) => {
                outln!(out, "✓ Pushed {} snapshot(s) and {} cue(s)",
                    push_count, push_cue_count);
                break;
            }
            Some(SyncMsg::Conflict { project, relay_head, push_head }) => {
                outln!(out, "⚠ Conflict detected on '{}': relay head {} vs our latest {}",
                    project,
            clavus_core::short(&relay_head, 8),
            clavus_core::short(&push_head, 8));
                outln!(out, "  Run `clavus pull --relay {}` first to merge.", relay);
                break;
            }
            Some(msg) => {
                outln!(out, "  relay: {:?}", msg);
            }
            None => {
                outln!(out, "⚠ Relay timed out waiting for acknowledgement");
                break;
            }
        }
    }

    Ok(())
}

pub fn cmd_pull(relay: &str, project: &Path, clavus_root: &Path,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> anyhow::Result<()> {
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

    outln!(out, "Connecting to {}...", relay);
    let client = SyncClient::connect(relay)
        .map_err(|e| anyhow::anyhow!("Failed to connect to {}: {}", relay, e))?;

    // Announce what we know
    client.hello(&name, known_heads)
        .map_err(|e| anyhow::anyhow!("Hello: {}", e))?;

    // Receive relay's response (Push with missing snapshots)
    let timeout = std::time::Duration::from_secs(10);
    let mut pulled_count = 0;
    let mut pulled_cues = 0;
    loop {
        match client.recv_timeout(timeout) {
            Some(SyncMsg::Push { snapshots, cues }) => {
                let mut newest: Option<String> = None;
                for snap in &snapshots {
                    if !snap_store.has(&snap.hash) {
                        snap_store.save_raw(snap.clone())?;
                        outln!(
                        out,
                        "  ↳ {}  {}",
                        clavus_core::short(&snap.hash, 8),
                        snap.message
                    );
                        pulled_count += 1;
                    }
                    // Track the newest pulled snapshot (relay sends root → head)
                    newest = Some(snap.hash.clone());
                }
                if snapshots.is_empty() {
                    outln!(out, "  Relay has no new snapshots");
                } else if let Some(hash) = newest {
                    // Advance HEAD to the pulled head so the chain is usable.
                    // A shrink is refused (that would be a divergent history),
                    // so report it instead of silently rewinding.
                    match snap_store.set_head(&hash, "pull") {
                        Ok(()) => outln!(out, "  HEAD → {}", clavus_core::short(&hash, 8)),
                        Err(e) => outln!(out, 
                            "  ⚠ Could not advance HEAD to {}: {}",
                            clavus_core::short(&hash, 8),
                            e
                        ),
                    }
                }
                // Import the collaborator's cues into the local cue store.
                pulled_cues += import_cues(&cues, clavus_root, &name, out, err);
                client.synced().ok();
                break;
            }
            Some(SyncMsg::Conflict { project, relay_head, push_head }) => {
                outln!(out, "⚠ Conflict detected on '{}'", project);
                outln!(out, "  relay HEAD: {}  your HEAD: {}",
                    clavus_core::short(&relay_head, 8),
            clavus_core::short(&push_head, 8));
                break;
            }
            Some(msg) => {
                outln!(out, "  relay: {:?}", msg);
            }
            None => {
                outln!(out, "⚠ Relay timed out — no response");
                break;
            }
        }
    }

    if pulled_count > 0 {
        outln!(out, "✓ Pulled {} new snapshot(s)", pulled_count);
        outln!(out, "  Note: sync transfers snapshot metadata, not project file contents.");
        outln!(out, "        Restoring a pulled snapshot isn't possible on this machine.");
    } else {
        outln!(out, "✓ Already up to date");
    }

    if pulled_cues > 0 {
        outln!(out, "✓ Imported {} new cue(s)", pulled_cues);
    }

    Ok(())
}

/// Import remote cues into the local cue store.
///
/// Returns the number of cues that were newly added. A cue that is already
/// present, or that conflicts with a newer local one, is reported but not
/// counted as new.
fn import_cues(
    cues: &[clavus_core::types::Cue],
    clavus_root: &Path,
    project_name: &str,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> usize {
    if cues.is_empty() {
        return 0;
    }
    let store = match clavus_core::CueStore::new(clavus_root, project_name) {
        Ok(s) => s,
        Err(e) => {
            errln!(err, "  ⚠ Could not open the cue store: {}", e);
            return 0;
        }
    };
    let mut imported = 0;
    for cue in cues {
        match store.import(cue.clone()) {
            Ok(clavus_core::cue::ImportOutcome::Imported) => {
                outln!(out, "  ✎ {}  {}", truncate(&cue.id, 8), truncate(&cue.text, 40));
                imported += 1;
            }
            Ok(clavus_core::cue::ImportOutcome::Unchanged) => {}
            Ok(clavus_core::cue::ImportOutcome::Conflict) => {
                outln!(out, "  ⚠ Conflict on cue {} — kept the local copy", truncate(&cue.id, 8));
            }
            Err(e) => {
                errln!(err, "  ⚠ Rejected cue {}: {}", truncate(&cue.id, 8), e);
            }
        }
    }
    imported
}

pub fn cmd_restore(project: &Path, hash_arg: &str, clavus_root: &Path,
    out: &mut dyn Write,
    _err: &mut dyn Write,
) -> anyhow::Result<()> {
    use clavus_core::blob::BlobStore;
    use clavus_core::snapshot::SnapshotStore;

    let name = project
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    let store = BlobStore::new(clavus_root.join("blobs"))
        .map_err(|e| anyhow::anyhow!("BlobStore: {}", e))?;
    let snap_store = SnapshotStore::new(store, &name);

    // Find the snapshot: HEAD keyword, or full hash / unique prefix.
    let target_hash = if hash_arg == "HEAD" {
        snap_store.head()
            .map(|h| h.hash)
            .ok_or_else(|| anyhow::anyhow!("No snapshots found for '{}'", name))?
    } else {
        snap_store.resolve(hash_arg).map_err(|e| match e {
            // A genuine ambiguity is worth reporting verbatim.
            clavus_core::ClavusError::AmbiguousSnapshot(_) => anyhow::anyhow!("{}", e),
            _ => anyhow::anyhow!(
                "No snapshot found matching '{}'. Run `clavus log {:?}` to see snapshot hashes.",
                hash_arg, project
            ),
        })?
    };

    outln!(
        out,
        "Restoring {} to snapshot {}...",
        name,
        clavus_core::short(&target_hash, 8)
    );

    let restored = snap_store.restore(&target_hash, Some(project))?;

    outln!(out, "✓ Restored to: {}", clavus_core::short(&restored.hash, 8));
    outln!(out, "  {}", restored.message);
    outln!(out, "  Restart Ableton/Bitwig to load the restored file.");

    Ok(())
}

pub fn cmd_status(project: &Path, clavus_root: &Path,
    out: &mut dyn Write,
    _err: &mut dyn Write,
) -> anyhow::Result<()> {
    use clavus_core::blob::BlobStore;
    use clavus_core::cue::CueStore;
    use clavus_core::snapshot::SnapshotStore;

    let name = project
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    let project_type = if project.extension().and_then(|s| s.to_str()) == Some("bwproject") {
        "Bitwig"
    } else {
        "Ableton Live"
    };

    outln!(out, "{name}  ({project_type})");
    outln!(out, "{}", "─".repeat(50));

    // ── Sample audit ──────────────────────────────────────────────
    let report = sample::audit_project(project)?;
    if report.is_clean() {
        outln!(out, "✓  {} samples present, all inside project folder", report.collected.len());
    } else {
        if !report.missing.is_empty() {
            outln!(out, "✗  {} sample(s) missing — run `clavus collect`", report.missing.len());
        }
        if !report.outside_project.is_empty() {
            outln!(out, "⚠  {} sample(s) outside project folder", report.outside_project.len());
        }
    }

    // ── Snapshots ───────────────────────────────────────────────
    let store = BlobStore::new(clavus_root.join("blobs"))
        .map_err(|e| anyhow::anyhow!("BlobStore: {}", e))?;
    let snap_store = SnapshotStore::new(store, &name);

    let head = snap_store.head();
    let count = snap_store.chain_length(None);
    if count == 0 {
        outln!(out, "📷  No snapshots yet — run `clavus snapshot`");
    } else {
        let h = head.as_ref().map(|s| s.short_hash(8)).unwrap_or_default();
        let msg = head.as_ref().map(|s| s.message.as_str()).unwrap_or_default();
        outln!(out, "📷  {count} snapshot(s)  (HEAD: {h} — {msg})");
    }

    // ── Cues ────────────────────────────────────────────────────
    let cue_store = CueStore::new(clavus_root, &name)?;
    let all_cues = cue_store.list(None)?;
    let pending = all_cues.iter().filter(|c| matches!(c.status, clavus_core::types::CueStatus::Pending)).count();
    let resolved = all_cues.len() - pending;
    if all_cues.is_empty() {
        outln!(out, "🎯  No cues");
    } else {
        outln!(out, "🎯  {total} cue(s)  ({pending} pending, {resolved} resolved)", total = all_cues.len());
    }

    Ok(())
}

pub fn cmd_projects(
    recursive: bool,
    dir: Option<&Path>,
    out: &mut dyn Write,
    _err: &mut dyn Write,
) -> anyhow::Result<()> {
    // An explicit directory wins over the standard locations. Ignoring it
    // made `clavus projects --dir <path>` scan the user's home folder
    // instead of the directory they asked for.
    let dirs: Vec<PathBuf> = match dir {
        Some(d) => vec![d.to_path_buf()],
        None => standard_albeton_dirs(),
    };
    if dirs.is_empty() {
        outln!(out, "No standard locations found. Pass --dir explicitly.");
        return Ok(());
    }

    let mut projects: Vec<(String, PathBuf)> = Vec::new();
    let mut visited = std::collections::HashSet::new();

    for base in dirs {
        find_projects_in(&base, recursive, &mut projects, &mut visited, 0);
    }

    if projects.is_empty() {
        outln!(out, "No Ableton/Bitwig projects found.");
        outln!(out, "Hint: pass --dir to scan a specific folder, or use clavus tui");
        return Ok(());
    }

    outln!(out, "Found {} project(s):\n", projects.len());
    for (name, path) in &projects {
        let modified = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .map(format_modified_time)
            .unwrap_or_default();

        outln!(out, "  {}  {}", name, modified);
        outln!(out, "  {}", path.display());
        outln!(out, );
    }
    Ok(())
}

pub fn standard_albeton_dirs() -> Vec<PathBuf> {
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

fn format_modified_time(t: std::time::SystemTime) -> String {
    // A file's timestamp can predate the Unix epoch — an archive with a
    // bogus time, or a volume with a bad clock. That must not panic.
    let Ok(dur) = t.duration_since(std::time::UNIX_EPOCH) else {
        return "????-??-?? ??:??".to_string();
    };
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
    let yday = days_left.max(0) as usize;
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

/// How deep a recursive project scan will go.
const MAX_SCAN_DEPTH: usize = 12;

fn find_projects_in(
    dir: &Path,
    recursive: bool,
    out: &mut Vec<(String, PathBuf)>,
    visited: &mut std::collections::HashSet<PathBuf>,
    depth: usize,
) {
    if depth > MAX_SCAN_DEPTH {
        return;
    }

    // A symlink that points at an ancestor makes the walk go in circles.
    // Canonicalising each directory and refusing to visit one twice stops
    // that instead of recursing until the stack runs out.
    if let Ok(real) = dir.canonicalize() {
        if !visited.insert(real) {
            return;
        }
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if recursive {
                find_projects_in(&path, recursive, out, visited, depth + 1);
            }
            continue;
        }

        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ext == "als" || ext == "bwproject" {
            if let Some(name) = path.file_stem().map(|s| s.to_string_lossy().to_string()) {
                out.push((name, path));
            }
        }
    }
}

pub fn projects_dir() -> PathBuf {
    dirs::audio_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn clavus_root() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".clavus"))
        .unwrap_or_else(|| PathBuf::from(".clavus"))
}

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

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
    #[arg(short, long, global = true)]
    pub dir: Option<PathBuf>,

    /// Clavus data directory (default: ~/.clavus, or $CLAVUS_DATA)
    #[arg(long, global = true)]
    pub data: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Launch the terminal UI
    Tui {
        /// Relay to sync with (e.g. 192.168.1.10:4477)
        #[arg(short = 'r', long)]
        relay: Option<String>,
    },
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
        /// Who wrote the note (default: your user name)
        #[arg(short, long)]
        author: Option<String>,
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
        /// Exit with an error if any sample is missing or lies outside
        /// the project folder. Without it the audit only reports.
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
        /// Only look at the top level of each location
        #[arg(long)]
        no_recursive: bool,
    },
    /// Show project health at a glance: snapshots, samples and cues
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
    /// Bring an old Clavus store into this one
    ImportLegacy {
        /// The old `~/.clavus` directory to read from
        #[arg(long)]
        store: PathBuf,
        /// Where the sets are now, if they are not where the old store
        /// said. The old store names each project by a path on the
        /// machine it was written on, which means nothing here.
        #[arg(long)]
        projects: Option<PathBuf>,
        /// Say what would happen without writing anything
        #[arg(long)]
        dry_run: bool,
    },
    /// Say where a path a set points at has moved to
    ///
    /// A set writes down the absolute path of every sample it uses. When
    /// a drive is renamed or a library moves, those paths stop resolving
    /// and the set looks like it has lost its audio. This is where Clavus
    /// is told what moved where.
    Relocate {
        #[command(subcommand)]
        action: RelocateAction,
    },
}

/// What to do with the relocation map.
#[derive(Subcommand, Debug, Clone)]
pub enum RelocateAction {
    /// Show every rule
    List,
    /// Say that a path has moved somewhere else
    Map {
        /// The path the set points at, as it was written down
        from: String,
        /// Where that path lives now
        to: String,
    },
    /// Forget a rule
    Unmap {
        /// The path the rule starts from
        from: String,
    },
    /// Work out which rules would put the missing samples back
    Suggest {
        /// The folder to search for the audio
        #[arg(long)]
        root: PathBuf,
        /// The sets to look at
        projects: Vec<PathBuf>,
        /// The folder of sets to look at, when they are not named
        #[arg(long)]
        dir: Option<PathBuf>,
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

    let id = clavus_core::project::ProjectId::new(project);

    let parser = if project.extension().and_then(|s| s.to_str()) == Some("bwproject") {
        clavus_parser_bw::BitwigParser::new(project)?.project
    } else {
        clavus_parser_als::AlsParser::new(project)?.project
    };

    let relocations = clavus_core::relocate::Relocations::load(clavus_root)?;
    let report = sample::audit_project_with(project, &relocations)?;
    if let Err(e) = report.ensure_clean() {
        errln!(err, "✗ {}", e);
        for line in e.detail_lines() {
            errln!(err, "{}", line);
        }
        if collect {
            errln!(err, "\n→ Running `clavus collect` to copy samples into project folder...");
            sample::collect_samples(project, &report)?;
            // Re-audit after collecting
            let report = sample::audit_project_with(project, &relocations)?;
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
    let snap_store = SnapshotStore::new(store.clone(), &id);

    // Parent: current HEAD
    let parent = snap_store.head().map(|s| s.hash);

    // Build sample info from audit (collected + outside_project, all with hashes)
    //
    // The path a sample is recorded under is the path it has *inside
    // the project*, so the same record means the same thing on the
    // machine that pulls it. A sample outside the project keeps its
    // absolute path: it is not the project's to keep.
    let project_dir = project.parent().unwrap_or(Path::new("."));
    let all_present: Vec<_> = report
        .collected
        .iter()
        .chain(report.outside_project.iter())
        .filter_map(|s| {
            let hash = s.hash.clone()?;
            let path = s.path.as_path();
            let recorded = clavus_core::snapshot::relative_inside(project_dir, path)
                .unwrap_or_else(|| path.to_string_lossy().to_string());
            Some((hash, recorded))
        })
        .collect::<Vec<_>>();
    let sample_hashes: Vec<_> = all_present.iter().map(|(h, _)| h.clone()).collect();
    let sample_paths: std::collections::HashMap<_, _> = all_present.into_iter().collect();

    let msg = message
        .as_deref()
        .unwrap_or("auto snapshot");

    let outcome = snap_store.save(
        project,
        msg,
        parent.as_deref(),
        parser.track_count,
        parser.bpm,
        sample_hashes,
        sample_paths,
    )?;

    let snapshot = match outcome {
        clavus_core::snapshot::SaveOutcome::Created(s) => s,
        clavus_core::snapshot::SaveOutcome::Updated(s) => {
            // The project file is the same, but the audio it points at
            // is not. The snapshot's manifest described audio that no
            // longer existed, so it was refreshed — with the new audio
            // and the new message.
            outln!(out,
                "✓ snapshot {}  ·  {}  ·  {} tracks  ·  {:.0} BPM",
                clavus_core::short(&s.hash, 8),
                format_timestamp(s.timestamp),
                s.track_count,
                s.bpm,
            );
            outln!(out, "  the project file is unchanged — the audio it points at changed, and the snapshot now holds the new audio");
            return Ok(());
        }
        clavus_core::snapshot::SaveOutcome::Unchanged(s) => {
            // The project's bytes are identical to a snapshot this project
            // already has. Nothing was stored and the message was not
            // recorded — say so rather than claim a new snapshot.
            outln!(out,
                "= snapshot {}  ·  unchanged — no new snapshot was created",
                clavus_core::short(&s.hash, 8),
            );
            outln!(out, "  (already in this project's history as \"{}\")", s.message);
            outln!(out, "  (the message \"{}\" was not recorded — change the project first)", msg);
            return Ok(());
        }
    };

    outln!(out, 
        "✓ snapshot {}  ·  {}  ·  {} tracks  ·  {:.0} BPM",
        clavus_core::short(&snapshot.hash, 8),
        format_timestamp(snapshot.timestamp),
        snapshot.track_count,
        snapshot.bpm,
    );
    Ok(())
}

/// Make sure the project the user named actually exists.
///
/// A path that does not exist used to be treated as an empty project:
/// `log` said "No snapshots yet", `cues` said "No cues" and `cue`
/// cheerfully created a cue for it. The same binary reported the
/// missing file as an error for `audit`, `status` and `snapshot`, so
/// the behaviour was inconsistent.
fn ensure_project_exists(project: &Path) -> anyhow::Result<()> {
    // A directory exists, so the order of the checks matters: a folder
    // of projects is not a project, and saying "no snapshots yet" about
    // it would be a lie about a project that was never there.
    if project.is_dir() {
        anyhow::bail!("{} is a directory, not a project file", project.display());
    }
    if project.exists() {
        return Ok(());
    }
    anyhow::bail!("the project {} does not exist", project.display());
}

/// A size in bytes, said the way a person would say it.
fn human_size(bytes: usize) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    let b = bytes as f64;
    if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{} bytes", bytes)
    }
}

/// The name to record as the author of a note.
///
/// The user's own name, so a collaborator can tell who wrote a note
/// after a sync. Falls back to "cli" when the name cannot be found.
pub fn default_author() -> String {
    clavus_core::default_author()
}

/// Check that a cue position is a `bars.beats.sixteenths` position.
///
/// A position that cannot be parsed is a cue that can never be found
/// in the DAW, so it is refused rather than stored.
fn validate_position(position: &str) -> anyhow::Result<()> {
    let parts: Vec<&str> = position.split('.').collect();
    let ok = parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
    if !ok {
        anyhow::bail!(
            "'{}' is not a valid position — expected bars.beats.sixteenths, e.g. 1.1.1",
            position
        );
    }
    Ok(())
}

pub fn cmd_log(project: &Path, clavus_root: &Path,
    out: &mut dyn Write,
    _err: &mut dyn Write,
) -> anyhow::Result<()> {
    use clavus_core::blob::BlobStore;
    use clavus_core::snapshot::SnapshotStore;

    ensure_project_exists(project)?;
    let id = clavus_core::project::ProjectId::new(project);

    let store = BlobStore::open(clavus_root.join("blobs"));
    // A store that cannot be read is not an empty store. Saying "no
    // snapshots" when the directory is unreadable tells the user their
    // history is empty when it is really hidden.
    if let Err(e) = store.check_readable() {
        anyhow::bail!("cannot read the Clavus data directory: {}", e);
    }
    let snap_store = SnapshotStore::new(store, &id);

    // A record that exists but cannot be read is a broken history, not
    // an empty one. Saying "no snapshots" would hide the damage.
    let corrupt = snap_store.corrupt_records();
    if !corrupt.is_empty() {
        anyhow::bail!(
            "the snapshot record for {} is corrupt and cannot be read — the history of this project cannot be shown until it is repaired",
            clavus_core::short(&corrupt[0], 8)
        );
    }

    let head = match snap_store.head() {
        Some(h) => h,
        None => {
            outln!(out, "No snapshots yet — run `clavus snapshot <path>` first.");
            return Ok(());
        }
    };

    let chain = snap_store.walk_chain(Some(&head.hash));
    // The clock is named, so a reader knows whose clock the times are on.
    outln!(
        out,
        "{} snapshot(s) in chain — times are local ({})\n",
        chain.len(),
        clavus_core::time::local_offset_label()
    );

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
            one_line(&s.message),
            parent,
            s.track_count,
            s.bpm,
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn cmd_cue(
    project: &Path,
    text: &str,
    position: &str,
    track: Option<&str>,
    author: Option<&str>,
    clavus_root: &Path,
    out: &mut dyn Write,
    _err: &mut dyn Write,
) -> anyhow::Result<()> {
    ensure_project_exists(project)?;
    validate_position(position)?;
    let root = clavus_root;
    let id = clavus_core::project::ProjectId::new(project);
    let store = clavus_core::CueStore::new(root, &id)?;
    // The author says who wrote the note. It used to be the constant
    // "cli", so after a sync a collaborator could not tell whose notes
    // were whose — which is the whole point of sharing notes.
    let author = author.map(|a| a.to_string()).unwrap_or_else(default_author);
    let cue = store.add(text, position, &author, "", track.unwrap_or("Arrangement"))?;
    outln!(out, "✓ cue {}  @ {}  ({})",
        &cue.id[..8.min(cue.id.len())],
        cue.position,
        cue.status.as_str());
    outln!(out, "  {}", truncate(&one_line(&cue.text), 60));
    Ok(())
}

pub fn cmd_cues(project: &Path, status: Option<&str>, clavus_root: &Path,
    out: &mut dyn Write,
    _err: &mut dyn Write,
) -> anyhow::Result<()> {
    ensure_project_exists(project)?;
    let root = clavus_root;
    let id = clavus_core::project::ProjectId::new(project);
    let store = clavus_core::CueStore::open(root, &id);
    let cues = store.list(status).map_err(|e| match e {
        // The status filter is what the user just typed, so the message
        // must not blame the project file.
        clavus_core::ClavusError::ParseError(msg) if msg.starts_with("unknown cue status") => {
            anyhow::anyhow!("{}", msg)
        }
        other => anyhow::anyhow!("Failed to read the cues: {}", other),
    })?;

    if cues.is_empty() {
        outln!(out, "No cues.");
    } else {
        for c in &cues {
            // A note is one line, whatever the user typed. A newline
            // in the text used to break the table apart, and an escape
            // sequence was handed straight to the terminal.
            let text = truncate(&one_line(&c.text), 22);
            // The track is part of the note: a note that says "drop the
            // bass here" is useless without knowing which track it is
            // on. The column is only shown when a note carries one, so
            // the common case stays quiet.
            let track = match c.track_name.as_str().trim() {
                t if !t.is_empty() => Some(truncate(&one_line(t), 14)),
                _ => None,
            };
            match track {
                Some(track) => outln!(out,
                    "  {}  [{:9}]  {:22}  @ {:>8}  {:14}  {}",
                    truncate(&c.id, 8),
                    c.status.as_str(),
                    text,
                    c.position,
                    track,
                    one_line(&c.author)
                ),
                None => outln!(out,
                    "  {}  [{:9}]  {:22}  @ {:>8}  {}",
                    truncate(&c.id, 8),
                    c.status.as_str(),
                    text,
                    c.position,
                    one_line(&c.author)
                ),
            }
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
    // The socket is bound *before* the tool claims to be listening.
    // Printing first meant a relay that could not start still said
    // "Listening on …" and then failed.
    let listener = std::net::TcpListener::bind(bind)
        .map_err(|e| anyhow::anyhow!("Cannot listen on {}: {}", bind, e))?;
    let bound = listener
        .local_addr()
        .map(|a| a.to_string())
        .unwrap_or_else(|_| bind.to_string());
    outln!(out, "Listening on {}…", bound);
    outln!(out, "State: {}", dir.display());
    clavus_relay::run_with_listener(listener, dir)?;
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

/// Make a piece of user text safe to print in a table.
///
/// A note is one line, whatever the user typed. A newline in the text
/// used to break the table apart — the second line looked like another
/// row, and the columns no longer lined up — and an escape sequence
/// was handed straight to the terminal, which drew whatever it said.
fn one_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => {
                // An escape sequence: CSI sequences run until a letter,
                // and the two-character ones stand on their own.
                if chars.peek() == Some(&'[') {
                    chars.next();
                    for c in chars.by_ref() {
                        if c.is_ascii_alphabetic() {
                            break;
                        }
                    }
                } else {
                    chars.next();
                }
            }
            '\n' | '\r' | '\t' => out.push(' '),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

/// Format a count with a correctly pluralised noun.
fn plural(count: usize, singular: &str, plural: &str) -> String {
    if count == 1 {
        format!("{} {}", count, singular)
    } else {
        format!("{} {}", count, plural)
    }
}

/// Print the sample summary.
///
/// `audit` and `status` used to print different numbers for the same
/// project: the audit counted the samples inside a `.bwproject` archive
/// as "present" while the status counted only the ones on disk. One
/// summary, used by both, keeps them honest and in agreement.
fn print_sample_summary(report: &clavus_core::SampleAuditReport, out: &mut dyn Write) {
    let needs_repair: Vec<_> = report
        .embedded
        .iter()
        .filter(|s| s.archive_needs_repair())
        .collect();
    let settled = report.embedded.len() - needs_repair.len();

    outln!(
        out,
        "✓ {} accounted for",
        plural(report.total(), "sample", "samples")
    );
    if !report.collected.is_empty() {
        outln!(
            out,
            "  {} on disk in the project folder",
            plural(report.collected.len(), "sample", "samples")
        );
    }
    if settled > 0 {
        outln!(
            out,
            "  {} inside the project archive",
            plural(settled, "sample", "samples")
        );
    }
    if !needs_repair.is_empty() {
        outln!(
            out,
            "  {} inside the project archive, but the project does not point at them — run `clavus collect`",
            plural(needs_repair.len(), "sample", "samples")
        );
        for s in &needs_repair {
            outln!(
                out,
                "    {} → {}",
                s.path.display(),
                s.archive_entry.as_deref().unwrap_or("?")
            );
        }
    }
}

pub fn cmd_audit(project: &Path, strict: bool,
    clavus_root: &Path,
    out: &mut dyn Write,
    _err: &mut dyn Write,
) -> anyhow::Result<()> {
    let relocations = clavus_core::relocate::Relocations::load(clavus_root)?;
    let report = sample::audit_project_with(project, &relocations)?;

    if report.is_clean() {
        print_sample_summary(&report, out);
    } else {
        // The audit's *result* belongs on stdout, like the clean
        // report and like `clavus status`. A script that reads stdout
        // must not find it empty exactly when there is something to
        // say. Errors — a project that could not be read — go to
        // stderr.
        if !report.missing.is_empty() {
            outln!(out, "✗ {} missing:", report.missing.len());
            for s in &report.missing {
                outln!(out, "  {}", s.path.display());
            }
        }
        if !report.outside_project.is_empty() {
            outln!(out, "✗ {} outside project:", report.outside_project.len());
            for s in &report.outside_project {
                outln!(out, "  {}", s.path.display());
                // The set points at a path that has moved; the audio is
                // here. Saying where is the difference between "lost"
                // and "not collected yet".
                if let Some(found) = &s.resolved_path {
                    if found != &s.path {
                        outln!(out, "    ↳ found at {}", found.display());
                    }
                }
            }
        }
        // Live's own factory samples are not the user's audio and are
        // not lost. They are named, so the user knows what the set
        // expects, but they do not fail the audit.
        if !report.factory.is_empty() {
            outln!(out, "  {} from Live's own library (not missing — they come back with Live):",
                report.factory.len());
        }
        if strict {
            return Err(anyhow::anyhow!("Audio audit failed"));
        }
    }
    Ok(())
}

pub fn cmd_collect(project: &Path, clavus_root: &Path,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> anyhow::Result<()> {
    let relocations = clavus_core::relocate::Relocations::load(clavus_root)?;
    let report = sample::audit_project_with(project, &relocations)?;

    if report.is_clean() && report.embedded.is_empty() {
        outln!(out, "✓ {} already present, all inside project folder",
            plural(report.total(), "sample", "samples"));
        return Ok(());
    }

    outln!(out, 
        "  {} missing  ·  {} outside project  ·  {} inside archive",
        report.missing.len(),
        report.outside_project.len(),
        report.embedded.len()
    );

    // Copy outside-project samples into project/Samples/ and rewrite paths
    let collect = sample::collect_samples(project, &report)?;

    if collect.repointed_in_project > 0 {
        // Nothing was copied: the audio was already in the project and
        // the set was still pointing at the machine it came from. That
        // is a repair, and it has to be said — "nothing to collect"
        // would leave the user thinking the set was left alone.
        outln!(
            out,
            "\n✓ Pointed {} at the copies already in the project",
            plural(collect.repointed_in_project, "reference", "references")
        );
        outln!(out, "  Nothing was copied — the samples were already there. Paths in .als rewritten — restart Ableton to reload");
    }

    if !collect.copied.is_empty() {
        let is_bitwig = project
            .extension()
            .and_then(|s| s.to_str())
            .map(|e| e.eq_ignore_ascii_case("bwproject"))
            .unwrap_or(false);
        if is_bitwig {
            // The bundle is the project: the samples went into it, or
            // they were in it already. Saying that a folder next to the
            // bundle was filled would be a lie — there is no such
            // folder.
            if collect.added_to_bundle > 0 {
                outln!(out, "\n✓ Collected {} into the project bundle",
                    plural(collect.added_to_bundle, "sample", "samples"));
                outln!(out, "  Paths in the project archive rewritten — reopen the project in Bitwig");
            } else {
                outln!(out, "\n✓ {} already inside the project bundle",
                    plural(collect.already_inside, "sample", "samples"));
                if collect.pointed_inside > 0 || collect.rewritten > 0 {
                    // The samples were in the bundle already; the
                    // references were not. Saying the paths "already
                    // point at the copies" would contradict what the
                    // command just did.
                    outln!(out, "  Paths in the project archive rewritten to point at the copies inside it — reopen the project in Bitwig");
                } else {
                    outln!(out, "  Paths in the project archive already point at the copies inside it");
                }
            }
        } else {
            outln!(out, "\n✓ Collected {} into project/Samples/",
                plural(collect.copied.len(), "sample", "samples"));
            if collect.rewritten == collect.copied.len() {
                outln!(out, "  Paths in .als rewritten — restart Ableton to reload");
            } else {
                // Some references are still pointing outside the
                // project. Saying the paths were rewritten would be a
                // lie, and the user has to know which ones are still
                // out there.
                outln!(
                    out,
                    "  ⚠ {} could not be rewritten — the project still points at the outside file:",
                    plural(collect.copied.len() - collect.rewritten, "reference", "references")
                );
            }
        }
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

    if collect.copied.is_empty() && collect.repointed_in_project == 0 {
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

    let id = clavus_core::project::ProjectId::new(project);
    let name = id.name.clone();

    // Load local snapshot store
    let store = BlobStore::new(clavus_root.join("blobs"))
        .map_err(|e| anyhow::anyhow!("BlobStore: {}", e))?;
    let snap_store = SnapshotStore::new(store.clone(), &id);

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

    // The cues travel with the snapshots — a collaborator's notes are
    // the point of the cue system. They are collected *before* the
    // early return below, because a collaborator who only leaves notes
    // and takes no snapshot still wants those notes to leave the
    // machine.
    //
    // A note that cannot be read is named, and the notes that *can* be
    // read still go. Silently shipping nothing while printing "✓
    // Pushed" would leave the user's notes on the machine and say
    // nothing about it.
    let (push_cues, broken_notes) = clavus_core::CueStore::open(clavus_root, &id)
        .list_partial(None)
        .map_err(|e| anyhow::anyhow!("Could not read the notes: {}", e))?;
    if !broken_notes.is_empty() {
        errln!(
            err,
            "⚠ {} could not be read and will not be sent:",
            plural(broken_notes.len(), "note", "notes")
        );
        for reason in &broken_notes {
            errln!(err, "  {}", reason);
        }
        errln!(err, "  The rest of the notes are still being sent.");
    }

    if snapshots.is_empty() && push_cues.is_empty() {
        outln!(out, "Nothing to push for '{}' — no snapshots and no cues", name);
        return Ok(());
    }
    if snapshots.is_empty() {
        outln!(out, "No local snapshots for '{}' — pushing the cues only", name);
    }

    outln!(out, "Connecting to {}...", relay);
    let client = SyncClient::connect(relay)
        .map_err(|e| anyhow::anyhow!("Failed to connect to {}: {}", relay, e))?;

    // Say hello with empty known_heads so relay sends us everything
    client.hello_in(
        id.sync_name(),
        vec![],
        id.folder_path(),
        clavus_core::blob::BlobStore::content_hash_of_file(project),
    )
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
            project_folder: s.project_folder.clone(),
        }
    }).collect();

    // The project bytes travel first, so the relay has them by the
    // time it learns about the snapshot. Without them a collaborator
    // could see that a snapshot existed but not restore it.
    let mut pushed_bytes = 0usize;
    for snap in &push_snaps {
        let Some(bytes) = store.get(&snap.hash) else {
            continue;
        };
        client
            .push_blob(id.sync_name(), &snap.hash, &bytes)
            .map_err(|e| anyhow::anyhow!("Could not send the project bytes: {}", e))?;
        pushed_bytes += bytes.len();
    }
    if pushed_bytes > 0 {
        outln!(out, "  sent the project bytes ({})", human_size(pushed_bytes));
    }

    // The audio travels too. A collaborator who receives only the
    // project file has a set that points at files that are not there,
    // and the restore cannot fix it.
    let mut pushed_audio = 0usize;
    let mut pushed_audio_count = 0usize;
    for snap in &push_snaps {
        for hash in snap.sample_hashes.iter() {
            if !store.has(hash) {
                continue;
            }
            let Some(bytes) = store.get(hash) else {
                continue;
            };
            client
                .push_blob(id.sync_name(), hash, &bytes)
                .map_err(|e| anyhow::anyhow!("Could not send the audio: {}", e))?;
            pushed_audio += bytes.len();
            pushed_audio_count += 1;
        }
    }
    if pushed_audio_count > 0 {
        outln!(
            out,
            "  sent the audio for {} sample(s) ({})",
            pushed_audio_count,
            human_size(pushed_audio)
        );
    }

    let push_count = push_snaps.len();
    let push_cue_count = push_cues.len();
    // The count is the whole local chain, not the number of snapshots
    // that are new to the relay — the relay keeps what it already has.
    // The wording says so, so the number is not read as "new".
    outln!(
        out,
        "Pushing the local chain — {} snapshot(s), {} cue(s)...",
        push_count,
        push_cue_count
    );
    client
        .push(push_snaps, push_cues)
        .map_err(|e| anyhow::anyhow!("Push: {}", e))?;

    // Wait for Synced or Conflict
    let timeout = std::time::Duration::from_secs(10);
    loop {
        match client.recv_timeout(timeout) {
            Some(SyncMsg::Synced) => {
                outln!(
                    out,
                    "✓ Pushed the local chain — {} snapshot(s) and {} cue(s) sent",
                    push_count, push_cue_count
                );
                break;
            }
            Some(SyncMsg::Conflict { project, relay_head, push_head }) => {
                errln!(err, "✗ Conflict detected on '{}': relay head {} vs our latest {}",
                    project,
            clavus_core::short(&relay_head, 8),
            clavus_core::short(&push_head, 8));
                errln!(err, "  Run `clavus pull --relay {}` first to merge.", relay);
                // Nothing was pushed. A script that runs `clavus push`
                // must be able to tell that.
                anyhow::bail!("the relay has a different history for '{}'", project);
            }
            Some(SyncMsg::BlobStored { .. }) => {
                // The relay confirming that it has the bytes. Nothing to
                // say — the user does not need the hash.
            }
            Some(SyncMsg::Error { message }) => {
                // The relay could not do what was asked. It did not
                // acknowledge the push, so the push did not happen.
                errln!(err, "✗ The relay reported a problem: {}", message);
                anyhow::bail!("the relay reported a problem: {}", message);
            }
            Some(msg) => {
                outln!(out, "  relay: {:?}", msg);
            }
            None => {
                errln!(err, "✗ The relay did not acknowledge the push");
                anyhow::bail!("the relay did not acknowledge the push");
            }
        }
    }

    // The snapshots and the readable notes are on the relay. A note
    // that could not be read is still on this machine, and a script
    // that runs `clavus push` has to be able to tell that.
    if !broken_notes.is_empty() {
        anyhow::bail!(
            "{} could not be read and were not sent",
            plural(broken_notes.len(), "note", "notes")
        );
    }

    Ok(())
}

/// Ask the relay for a snapshot's bytes.
///
/// Returns `None` when the relay does not have them, or does not
/// answer. A snapshot without its bytes can still be seen in the log,
/// but it cannot be restored on this machine.
fn fetch_blob(
    client: &SyncClient,
    id: &clavus_core::project::ProjectId,
    hash: &str,
) -> Option<Vec<u8>> {
    client.request_blob(id.sync_name(), hash).ok()?;
    match client.recv_blob(id.sync_name(), hash, std::time::Duration::from_secs(30)) {
        Ok(Ok(bytes)) => Some(bytes),
        Ok(Err(e)) => {
            tracing::debug!("the relay did not send the bytes for {}: {}", hash, e);
            None
        }
        Err(e) => {
            tracing::debug!("the blob transfer failed for {}: {}", hash, e);
            None
        }
    }
}

/// Fetch the audio a pulled snapshot recorded.
///
/// The bytes are stored under their own hash, together with the path
/// the sample has inside the project, so a restore can put them back
/// where the project expects them.
fn pull_audio(
    client: &SyncClient,
    id: &clavus_core::project::ProjectId,
    store: &clavus_core::blob::BlobStore,
    snap: &clavus_core::types::Snapshot,
    out: &mut dyn Write,
) -> usize {
    let mut fetched = 0usize;
    for hash in snap.sample_hashes.iter() {
        if store.has(hash) {
            continue;
        }
        let Some(bytes) = fetch_blob(client, id, hash) else {
            continue;
        };
        // The path the sample has inside the project. A path that is
        // not a plain relative path is refused: it would write outside
        // the project.
        let relative = snap
            .sample_paths
            .get(hash)
            .map(|p| p.replace('\\', "/"))
            .filter(|p| {
                !p.is_empty()
                    && !p.starts_with('/')
                    && !p.contains(':')
                    && !p.split('/').any(|part| part == ".." || part.is_empty())
            });
        let filename = relative
            .as_deref()
            .and_then(|p| p.rsplit('/').next())
            .unwrap_or("sample");
        match store.put_sample_bytes(&bytes, filename, relative.as_deref().unwrap_or("")) {
            Ok(_) => fetched += 1,
            Err(e) => {
                outln!(out, "    (could not store the audio {}: {})", hash, e);
            }
        }
    }
    fetched
}

pub fn cmd_pull(relay: &str, project: &Path, clavus_root: &Path,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> anyhow::Result<()> {
    use clavus_core::blob::BlobStore;
    use clavus_core::snapshot::SnapshotStore;
    use clavus_sync::SyncMsg;

    let id = clavus_core::project::ProjectId::new(project);

    // Load local snapshot store
    let store = BlobStore::new(clavus_root.join("blobs"))
        .map_err(|e| anyhow::anyhow!("BlobStore: {}", e))?;
    let snap_store = SnapshotStore::new(store.clone(), &id);

    // Tell the relay everything we already have, not just our HEAD.
    // Sending only the HEAD made the relay offer the whole history back,
    // and the pull then moved HEAD to the oldest snapshot it received.
    let known_heads: Vec<String> = snap_store
        .walk_chain(None)
        .iter()
        .map(|s| s.hash.clone())
        .collect();
    let local_head = snap_store.head().map(|s| s.hash);

    outln!(out, "Connecting to {}...", relay);
    let client = SyncClient::connect(relay)
        .map_err(|e| anyhow::anyhow!("Failed to connect to {}: {}", relay, e))?;

    // Announce what we know
    client.hello_in(
        id.sync_name(),
        known_heads,
        id.folder_path(),
        clavus_core::blob::BlobStore::content_hash_of_file(project),
    )
    .map_err(|e| anyhow::anyhow!("Hello: {}", e))?;

    // Receive relay's response (Push with missing snapshots)
    let timeout = std::time::Duration::from_secs(10);
    let mut pulled_count = 0;
    let mut pulled_cues = 0;
    // How many of the pulled snapshots came with the project bytes.
    let mut pulled_with_bytes = 0;
    let mut pulled_without_bytes = 0;
    // A pull that never got an answer is a failure, not a success.
    let mut got_response = false;
    // A conflict is an answer, but not a successful pull.
    let mut saw_conflict = false;
    // A relay that reports a problem is not a successful pull either.
    let mut saw_error = false;
    // The bytes of the newest snapshot that arrived, if any.
    let mut project_bytes: Option<Vec<u8>> = None;
    loop {
        match client.recv_timeout(timeout) {
            Some(SyncMsg::Push { snapshots, cues }) => {
                got_response = true;
                for snap in &snapshots {
                    if !snap_store.has(&snap.hash) {
                        // The bytes come first, so the snapshot can
                        // really be restored on this machine.
                        let bytes = fetch_blob(&client, &id, &snap.hash);
                        match bytes {
                            Some(data) => {
                                store.put(&data);
                                pulled_with_bytes += 1;
                                // The bytes of the newest snapshot that
                                // arrived. If the project file is not
                                // here, this is the project — and it has
                                // to be written, or the snapshot cannot
                                // be restored on this machine.
                                project_bytes = Some(data);
                            }
                            None => {
                                pulled_without_bytes += 1;
                                outln!(
                                    out,
                                    "    (the relay has no bytes for this snapshot — it can be seen but not restored here)"
                                );
                            }
                        }
                        // The audio the snapshot recorded comes too, so
                        // the restored project plays.
                        pull_audio(&client, &id, &store, snap, out);
                        // The snapshot now lives here, so it records
                        // the local path. A collaborator's machine has a
                        // different path for the same project, and the
                        // restore has to work on this one.
                        let mut local = snap.clone();
                        local.project_path = id.path.to_string_lossy().to_string();
                        snap_store.save_raw(local)?;
                        outln!(
                        out,
                        "  ↳ {}  {}",
                        clavus_core::short(&snap.hash, 8),
                        snap.message
                    );
                        pulled_count += 1;
                    }
                }
                if snapshots.is_empty() {
                    outln!(out, "  Relay has no new snapshots");
                } else {
                    // Advance HEAD only when the pulled history *extends*
                    // ours. Moving HEAD to a snapshot that is not a
                    // descendant of the current HEAD would drop the
                    // snapshots the user already had out of the history.
                    let candidates: Vec<String> = snapshots
                        .iter()
                        .map(|s| s.hash.clone())
                        .filter(|h| snap_store.has(h))
                        .collect();
                    // The head must be the *newest* of the pulled
                    // snapshots. Picking the first one that extends the
                    // chain put the head on the oldest snapshot of a
                    // fresh client's first pull, and the newer
                    // snapshots then went missing from the log.
                    let new_head = newest_of(&snap_store, local_head.as_deref(), &candidates);
                    match new_head {
                        Some(hash) => match snap_store.set_head(&hash, "pull") {
                            Ok(()) => {
                                outln!(out, "  HEAD → {}", clavus_core::short(&hash, 8))
                            }
                            Err(e) => outln!(out,
                                "  ⚠ Could not advance HEAD to {}: {}",
                                clavus_core::short(&hash, 8),
                                e
                            ),
                        },
                        None => {
                            // The relay offering the snapshot we already
                            // have is not a divergence: nothing is wrong
                            // with the history, it simply does not move.
                            // Warning about it would send the user to
                            // fix a problem that does not exist.
                            let same = local_head
                                .as_deref()
                                .map(|h| candidates.iter().any(|c| c == h))
                                .unwrap_or(false);
                            if same {
                                outln!(out,
                                    "  ✓ The relay's history is the one you already have — HEAD stays at {}",
                                    local_head
                                        .as_deref()
                                        .map(|h| clavus_core::short(h, 8))
                                        .unwrap_or_else(|| "nothing".to_string())
                                );
                            } else {
                                outln!(out,
                                    "  ⚠ The relay's history does not extend yours — HEAD was left at {}",
                                    local_head
                                        .as_deref()
                                        .map(|h| clavus_core::short(h, 8))
                                        .unwrap_or_else(|| "nothing".to_string())
                                );
                            }
                        }
                    }
                }
                // Import the collaborator's cues into the local cue store.
                pulled_cues += import_cues(&cues, clavus_root, &id, out, err);
                client.synced().ok();
                break;
            }
            Some(SyncMsg::Conflict { project, relay_head, push_head }) => {
                got_response = true;
                outln!(out, "⚠ Conflict detected on '{}'", project);
                outln!(out, "  relay HEAD: {}  your HEAD: {}",
                    clavus_core::short(&relay_head, 8),
            clavus_core::short(&push_head, 8));
                // The relay's history and ours have diverged. Nothing
                // was pulled, and saying "up to date" would be a lie.
                saw_conflict = true;
                break;
            }
            Some(SyncMsg::Error { message }) => {
                // The relay could not do what was asked. It said so
                // instead of sending a history, so nothing was pulled.
                got_response = true;
                errln!(err, "✗ The relay reported a problem: {}", message);
                saw_error = true;
                break;
            }
            Some(msg) => {
                // Some other message — a "synced", an acknowledgement.
                // It says nothing about the project's history, so the
                // pull keeps waiting for the real answer.
                outln!(out, "  relay: {:?}", msg);
            }
            None => {
                outln!(out, "✗ No response from the relay — nothing was checked");
                break;
            }
        }
    }

    if !got_response {
        // The user is not up to date — nothing was even checked. Saying
        // otherwise is a lie that hides a broken connection.
        anyhow::bail!("the relay did not answer");
    }

    if saw_conflict {
        // The histories have diverged. The pull brought nothing, and
        // the user has to merge before anything can be pushed.
        anyhow::bail!("the relay's history and yours have diverged");
    }

    if saw_error {
        // The relay could not do what was asked. Nothing was pulled,
        // and saying "up to date" would hide the failure.
        anyhow::bail!("the relay reported a problem");
    }

    if pulled_count > 0 {
        outln!(out, "✓ Pulled {} new snapshot(s)", pulled_count);
        if pulled_with_bytes > 0 {
            // A snapshot can only be restored where the project file
            // lives. When the project is not here yet, the bytes that
            // arrived are the project — so they are written, and the
            // claim below is true.
            let written = match &project_bytes {
                Some(bytes) if !project.exists() => {
                    match std::fs::write(project, bytes) {
                        Ok(()) => {
                            outln!(
                                out,
                                "  Wrote the project file — it was not on this machine yet."
                            );
                            true
                        }
                        Err(e) => {
                            errln!(
                                err,
                                "  ✗ Could not write the project file {}: {}",
                                project.display(),
                                e
                            );
                            false
                        }
                    }
                }
                _ => project.exists(),
            };
            if written {
                outln!(
                    out,
                    "  {} of them came with the project bytes — they can be restored here.",
                    pulled_with_bytes
                );
            } else {
                outln!(
                    out,
                    "  {} of them came with the project bytes — they can be seen, but not restored until the project file exists.",
                    pulled_with_bytes
                );
            }
        }
        if pulled_without_bytes > 0 {
            outln!(
                out,
                "  {} came without the project bytes — they can be seen but not restored here.",
                pulled_without_bytes
            );
        }
    } else {
        // "Already up to date" was also printed when the relay had never
        // heard of this project, which reads as if the relay had checked
        // and found nothing to give. The honest wording covers both.
        if pulled_cues > 0 {
            outln!(
                out,
                "✓ Up to date — the relay had no new snapshots for this project; {} imported",
                plural(pulled_cues, "cue", "cues")
            );
        } else {
            outln!(out, "✓ Up to date — the relay had nothing new for this project");
        }
    }

    if pulled_cues > 0 && pulled_count > 0 {
        outln!(out, "✓ Imported {} new cue(s)", pulled_cues);
    }

    Ok(())
}

/// Whether `candidate` extends the chain that ends at `head`.
///
/// A snapshot extends the chain when the chain's head is one of its
/// ancestors — that is, when the candidate was made *after* the head.
/// Anything else is a different history, and moving HEAD to it would
/// drop the snapshots the user already had.
/// The newest of the pulled snapshots that extends the local chain.
///
/// The head must be the tip of the pulled history. Picking the first
/// candidate that extended the chain put a fresh client's head on the
/// *oldest* snapshot, and the newer ones went missing from the log.
fn newest_of(
    snap_store: &clavus_core::snapshot::SnapshotStore,
    local_head: Option<&str>,
    candidates: &[String],
) -> Option<String> {
    // The candidates that extend what we already have.
    let extending: Vec<String> = candidates
        .iter()
        .filter(|h| extends_chain(snap_store, local_head, h))
        .cloned()
        .collect();
    if extending.is_empty() {
        return None;
    }

    // The tip of the pulled history: the snapshot that is nobody's
    // parent.
    let snaps: Vec<clavus_core::types::Snapshot> = extending
        .iter()
        .filter_map(|h| snap_store.load(h))
        .collect();
    clavus_core::snapshot::newest_in(&snaps)
}

fn extends_chain(
    snap_store: &clavus_core::snapshot::SnapshotStore,
    head: Option<&str>,
    candidate: &str,
) -> bool {
    let Some(head) = head else {
        // No local history yet — anything is an extension.
        return true;
    };
    if head == candidate {
        return false;
    }
    // Walk the candidate's ancestry and look for our head.
    let mut current = Some(candidate.to_string());
    let mut seen = std::collections::HashSet::new();
    while let Some(hash) = current {
        if !seen.insert(hash.clone()) {
            return false;
        }
        if hash == head {
            return true;
        }
        current = snap_store.load(&hash).and_then(|s| s.parent);
    }
    false
}

/// Import remote cues into the local cue store.
///
/// Returns the number of cues that were newly added. A cue that is already
/// present, or that conflicts with a newer local one, is reported but not
/// counted as new.
fn import_cues(
    cues: &[clavus_core::types::Cue],
    clavus_root: &Path,
    project: &clavus_core::project::ProjectId,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> usize {
    if cues.is_empty() {
        return 0;
    }
    let store = match clavus_core::CueStore::new(clavus_root, project) {
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
    err: &mut dyn Write,
) -> anyhow::Result<()> {
    use clavus_core::blob::BlobStore;
    use clavus_core::snapshot::SnapshotStore;

    ensure_project_exists(project)?;

    let id = clavus_core::project::ProjectId::new(project);
    let name = id.name.clone();

    let store = BlobStore::new(clavus_root.join("blobs"))
        .map_err(|e| anyhow::anyhow!("BlobStore: {}", e))?;
    let snap_store = SnapshotStore::new(store.clone(), &id);

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

    // The audio the snapshot recorded goes back where the project
    // expects it. A file that is already there is left alone: the
    // user's own audio is not the tool's to replace.
    let audio = snap_store.restore_samples(&restored, project);
    if !audio.restored.is_empty() {
        outln!(
            out,
            "  ✓ Restored {} sample(s) the project points at.",
            audio.restored.len()
        );
    }
    if !audio.kept.is_empty() {
        outln!(
            out,
            "  {} sample(s) were already on disk — they were left as they are.",
            audio.kept.len()
        );
    }

    // A snapshot that holds no audio cannot bring it back, and the
    // user has to hear it from the tool rather than from Ableton
    // refusing to play.
    if !audio.missing.is_empty() {
        errln!(
            err,
            "  ⚠ {} of the recorded sample(s) are not on disk — the audio was not captured with this snapshot:",
            audio.missing.len()
        );
        for path in audio.missing.iter().take(5) {
            errln!(err, "      {}", path.display());
        }
        if audio.missing.len() > 5 {
            errln!(err, "      … and {} more", audio.missing.len() - 5);
        }
    }

    // Restoring an older version leaves the working file behind the
    // history. The newer snapshots are not lost — but the user has to
    // be told, because the file on disk and the head of the chain no
    // longer agree.
    if let Some(head) = snap_store.head() {
        if head.hash != target_hash {
            outln!(
                out,
                "  The chain still holds the newer snapshot {} — run `clavus snapshot` to record the restored state.",
                clavus_core::short(&head.hash, 8)
            );
        }
    }

    Ok(())
}

pub fn cmd_status(project: &Path, clavus_root: &Path,
    out: &mut dyn Write,
    _err: &mut dyn Write,
) -> anyhow::Result<()> {
    use clavus_core::blob::BlobStore;
    use clavus_core::cue::CueStore;
    use clavus_core::snapshot::SnapshotStore;

    let id = clavus_core::project::ProjectId::new(project);
    let name = id.name.clone();

    let project_type = if project.extension().and_then(|s| s.to_str()) == Some("bwproject") {
        "Bitwig"
    } else {
        "Ableton Live"
    };

    // ── Sample audit ──────────────────────────────────────────────
    //
    // The audit runs first: a report for a project that could not be
    // read is a report about nothing.
    let relocations = clavus_core::relocate::Relocations::load(clavus_root)?;
    let report = sample::audit_project_with(project, &relocations)?;

    outln!(out, "{name}  ({project_type})");
    outln!(out, "{}", "─".repeat(50));

    if report.is_clean() {
        print_sample_summary(&report, out);
    } else {
        if !report.missing.is_empty() {
            outln!(out, "✗  {} sample(s) missing — run `clavus collect`", report.missing.len());
        }
        if !report.outside_project.is_empty() {
            outln!(out, "⚠  {} sample(s) outside project folder", report.outside_project.len());
        }
    }

    // ── Snapshots ───────────────────────────────────────────────
    let store = BlobStore::open(clavus_root.join("blobs"));
    let snap_store = SnapshotStore::new(store, &id);

    let head = snap_store.head();
    let count = snap_store.chain_length(None);
    // A record that exists but cannot be read is a broken history. It
    // is reported as such — the same way `clavus log` reports it — and
    // not as a clean count.
    let corrupt = snap_store.corrupt_records();
    if !corrupt.is_empty() {
        outln!(
            out,
            "📷  ⚠ {} snapshot record(s) are corrupt and cannot be read",
            corrupt.len()
        );
        outln!(
            out,
            "     the first is {} — the history of this project is damaged",
            clavus_core::short(&corrupt[0], 8)
        );
    } else if count == 0 {
        outln!(out, "📷  No snapshots yet — run `clavus snapshot`");
    } else {
        let h = head.as_ref().map(|s| s.short_hash(8)).unwrap_or_default();
        let msg = head.as_ref().map(|s| s.message.as_str()).unwrap_or_default();
        outln!(out, "📷  {count} snapshot(s)  (HEAD: {h} — {msg})");
    }

    // ── Cues ────────────────────────────────────────────────────
    let cue_store = CueStore::open(clavus_root, &id);
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

    let projects = clavus_core::project::discover_projects(&dirs, recursive);

    if projects.is_empty() {
        outln!(out, "No Ableton/Bitwig projects found.");
        outln!(out, "Hint: pass --dir to scan a specific folder, or use clavus tui");
        return Ok(());
    }

    outln!(out, "Found {} project(s):\n", projects.len());
    for (name, path) in projects.iter().map(|p| (&p.name, &p.path)) {
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

/// The places a project is looked for when the user has not said where.
///
/// `~/Music` and `~/Documents` are where Ableton and most people keep
/// their sets. `~/Clavus/Projects` is where **Clavus itself** kept them
/// — the Python version's default — and a machine that was set up with
/// it still has that folder. The rewrite stopped looking there, so a
/// library that had been Clavus's own for years became invisible.
pub fn standard_albeton_dirs() -> Vec<PathBuf> {
    standard_albeton_dirs_in(dirs::audio_dir().as_deref(), dirs::home_dir().as_deref())
}

/// The standard places, given the music folder and the home folder.
///
/// The home folder is a parameter so the list can be tested without
/// changing the environment of the machine running the test.
pub fn standard_albeton_dirs_in(
    music: Option<&Path>,
    home: Option<&Path>,
) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(music) = music {
        dirs.push(music.to_path_buf());
    }
    if let Some(home) = home {
        let documents = home.join("Documents");
        if documents.exists() {
            dirs.push(documents);
        }
        // The original Clavus library.
        let clavus = home.join("Clavus").join("Projects");
        if clavus.exists() {
            dirs.push(clavus);
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
    // The local clock, like every other time the tool shows.
    clavus_core::time::format_local(dur.as_secs_f64())
}


pub fn projects_dir() -> PathBuf {
    dirs::audio_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Resolve a project argument against the projects directory.
///
/// The flag is called "projects directory", so a user who sets it
/// expects `clavus --dir ~/projects log Alpha.als` to find the
/// project — which usually lives in a folder below it. A path that
/// exists is used exactly as it was given: the flag never overrides
/// an explicit path.
///
/// Without the flag, the standard places are searched, so
/// `clavus log "My Set.als"` finds the set from anywhere. The flag, when
/// it is given, is the only place searched — a user who named a
/// directory did not ask for the whole library.
///
/// When several projects below the directory have the name, none of
/// them is chosen. Guessing would open the wrong project, and the
/// command says so instead.
pub fn resolve_project(project: &Path, dir: Option<&Path>) -> PathBuf {
    if project.exists() {
        return project.to_path_buf();
    }
    let dirs: Vec<PathBuf> = match dir {
        Some(d) => vec![d.to_path_buf()],
        None => standard_albeton_dirs(),
    };
    for dir in &dirs {
        let candidate = dir.join(project);
        if candidate.exists() {
            return candidate;
        }
    }
    // The name has to be unique across every place searched. Two sets
    // of the same name in two different folders are two projects, and
    // neither is the one that was asked for.
    let mut found: Option<PathBuf> = None;
    for dir in &dirs {
        if let Some(p) = find_unique_below(dir, project) {
            if found.is_some() {
                return project.to_path_buf(); // ambiguous
            }
            found = Some(p);
        }
    }
    found.unwrap_or_else(|| project.to_path_buf())
}

/// Find the one project below `dir` that answers to `name`.
///
/// Only `.als` and `.bwproject` files count, and the name has to match
/// exactly. Several matches are no match: the caller is better off with
/// an error than with the wrong project.
fn find_unique_below(dir: &Path, name: &Path) -> Option<PathBuf> {
    let matches = clavus_core::project::find_projects_named(&[dir.to_path_buf()], name);
    if matches.len() == 1 {
        Some(matches.into_iter().next().unwrap())
    } else {
        None
    }
}

pub fn clavus_root() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".clavus"))
        .unwrap_or_else(|| PathBuf::from(".clavus"))
}

/// A snapshot's time, in the user's own time zone.
///
/// The times used to be printed in UTC with no label, so they did not
/// match the user's clock and there was no way to tell what they were.
fn format_timestamp(secs: f64) -> String {
    clavus_core::time::format_local(secs)
}


/// Bring an old Clavus store into this one.
///
/// The older version kept its library in `~/Clavus/Projects` and its
/// history in `~/.clavus`. Its snapshot records carry the same fields
/// the current ones do, so the history can be brought across rather
/// than lost.
///
/// What cannot be done is guessed at: a project whose file is not found
/// is named and left behind, and a record that cannot be read is named
/// too. A migration that quietly dropped half the history would be
/// worse than one that said what it could not do.
pub fn cmd_import_legacy(
    store: &Path,
    projects: Option<&Path>,
    dry_run: bool,
    clavus_root: &Path,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> anyhow::Result<()> {
    use clavus_core::legacy::LegacyMigration;

    if !store.exists() {
        errln!(err, "Error: no old store at {}", store.display());
        return Ok(());
    }
    if !store.join("index.json").exists()
        && !store.join("index.json.bak").exists()
        && !store.join("index.json.bak2").exists()
        && !store.join("index.json.bak3").exists()
    {
        errln!(
            err,
            "Error: {} has no project index — it does not look like an old Clavus store",
            store.display()
        );
        return Ok(());
    }

    let home = dirs::home_dir();
    outln!(out, "Reading the old store at {}", store.display());
    if dry_run {
        outln!(out, "  (dry run — nothing will be written)");
    }

    let migration = LegacyMigration {
        old_store: store,
        new_store: clavus_root,
        projects_root: projects,
        home: home.as_deref(),
        dry_run,
    };
    let report = migration.run()?;

    if !report.imported.is_empty() {
        outln!(out, "");
        outln!(out, "Brought across:");
        for project in &report.imported {
            outln!(
                out,
                "  {} — {} snapshot(s), {} note(s)",
                project.name, project.snapshots, project.cues
            );
            outln!(out, "      {}", project.project_path.display());
        }
    }

    if !report.skipped.is_empty() {
        outln!(out, "");
        outln!(out, "Left behind:");
        for project in &report.skipped {
            outln!(out, "  {} — {}", project.name, project.reason);
        }
    }

    if !report.unreadable_snapshots.is_empty() {
        outln!(out, "");
        outln!(out, "Snapshot records that could not be read:");
        for path in &report.unreadable_snapshots {
            outln!(out, "  {}", path);
        }
    }

    if !report.unreadable_cues.is_empty() {
        outln!(out, "");
        outln!(out, "Notes that could not be read:");
        for path in &report.unreadable_cues {
            outln!(out, "  {}", path);
        }
    }

    outln!(out, "");
    if dry_run {
        outln!(
            out,
            "{} — nothing was written",
            report.summary().replace("blob(s) copied", "blob(s) would be copied")
        );
    } else {
        outln!(out, "{}", report.summary());
    }

    if report.imported.is_empty() && !report.skipped.is_empty() {
        errln!(
            err,
            "Error: nothing could be brought across — the old store's projects were not found"
        );
        return Ok(());
    }
    Ok(())
}

// ─── Relocation map ───────────────────────────────────────────────────────

/// Show, add or remove the "this path moved there" rules.
///
/// The map is what keeps a set that was made on another machine — or
/// before a drive was renamed — from looking like it has lost its audio.
pub fn cmd_relocate(
    action: &RelocateAction,
    clavus_root: &Path,
    out: &mut dyn Write,
    _err: &mut dyn Write,
) -> anyhow::Result<()> {
    use clavus_core::relocate::{is_safe_source, is_safe_target, Relocations};

    let mut map = match Relocations::load(clavus_root) {
        Ok(map) => map,
        Err(e) => anyhow::bail!("{}", e),
    };

    match action {
        RelocateAction::List => {
            if map.entries.is_empty() {
                outln!(out, "No rules. A set that points at a path that has moved");
                outln!(
                    out,
                    "is asked about them: `clavus relocate map <old path> <new path>`"
                );
                return Ok(());
            }
            outln!(out, "{} rule(s):", map.entries.len());
            for entry in &map.entries {
                outln!(out, "  {}  →  {}", entry.from, entry.to);
            }
        }
        RelocateAction::Map { from, to } => {
            if !is_safe_source(from) {
                anyhow::bail!("the path a set points at cannot contain `..`");
            }
            if !is_safe_target(to) {
                anyhow::bail!("where it went has to be an absolute path on this machine, and cannot contain `..`");
            }
            map.map(from, to)?;
            map.save(clavus_root)?;
            outln!(out, "✓ {}  →  {}", from.trim_end_matches('/'), to.trim_end_matches('/'));
            outln!(
                out,
                "  Clavus will look there when a set points at the old path."
            );
        }
        RelocateAction::Unmap { from } => {
            if map.unmap(from) {
                map.save(clavus_root)?;
                outln!(out, "✓ Removed the rule for {}", from.trim_end_matches('/'));
            } else {
                anyhow::bail!("no rule for {}", from.trim_end_matches('/'));
            }
        }
        RelocateAction::Suggest {
            root,
            projects,
            dir,
        } => {
            suggest_relocations(projects, dir.as_deref(), root, &map, out)?;
        }
    }
    Ok(())
}

/// Work out which rules would put the missing samples back.
///
/// Nothing is written. The rules are printed for the user to accept,
/// because a rule that is wrong would send a sample to the wrong place
/// — and only the user knows which copy of a sample is the right one.
fn suggest_relocations(
    projects: &[PathBuf],
    dir: Option<&Path>,
    root: &Path,
    map: &clavus_core::relocate::Relocations,
    out: &mut dyn Write,
) -> anyhow::Result<()> {
    use clavus_core::relocate::suggest_rules;

    if !root.exists() {
        anyhow::bail!("{} does not exist", root.display());
    }

    // The sets to look at.
    let sets: Vec<PathBuf> = if !projects.is_empty() {
        projects.to_vec()
    } else {
        let dirs: Vec<PathBuf> = match dir {
            Some(d) => vec![d.to_path_buf()],
            None => standard_albeton_dirs(),
        };
        if dirs.is_empty() {
            anyhow::bail!("no standard locations found — name the sets to look at");
        }
        outln!(
            out,
            "Looking at every set in {}. This takes a while.",
            dirs[0].display()
        );
        clavus_core::project::discover_projects(&dirs, true)
            .into_iter()
            .map(|p| p.path)
            .collect()
    };

    // Every reference that does not resolve.
    let mut references: Vec<PathBuf> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (n, set) in sets.iter().enumerate() {
        let report = match sample::audit_project_with(set, map) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for missing in &report.missing {
            if seen.insert(missing.path.clone()) {
                references.push(missing.path.clone());
            }
        }
        if sets.len() > 20 && (n + 1) % 20 == 0 {
            outln!(out, "  audited {} of {} sets…", n + 1, sets.len());
        }
    }

    if references.is_empty() {
        outln!(out, "Nothing is missing — there is nothing to suggest.");
        return Ok(());
    }
    outln!(
        out,
        "{} distinct reference(s) do not resolve. Searching {}…",
        references.len(),
        root.display()
    );

    let suggestions = suggest_rules(&references, root);

    if suggestions.rules.is_empty() {
        outln!(out, "No rule could be worked out.");
    } else {
        outln!(
            out,
            "\nThese rules would recover {} reference(s):\n",
            suggestions
                .rules
                .iter()
                .map(|r| r.references)
                .sum::<usize>()
        );
        for rule in &suggestions.rules {
            outln!(
                out,
                "  {:5}  clavus relocate map {:?} {:?}",
                rule.references,
                rule.from,
                rule.to
            );
        }
    }
    outln!(
        out,
        "\n{} reference(s) have more than one file of that name — no rule was proposed for them, because the wrong copy is worse than none.",
        suggestions.ambiguous
    );
    outln!(
        out,
        "{} reference(s) have no file of that name under {} — they are gone.",
        suggestions.unmatched,
        root.display()
    );
    Ok(())
}

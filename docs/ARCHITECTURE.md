# Clavus Architecture

**Version:** v0.1.0-beta  
**Repository:** `/Users/slowhands/Developer/clavus`  
**Tailscale MagicDNS:** `slow-hands-studio-1` (Chris's Mac) · `chrispc.tail46b8d9.ts.net` (Steven's PC)  
**Relay Ports:** `7891` (local relay process) → `7890` (Tailscale serve proxy)

---

## 1. System Overview

**Clavus** is a version control and collaboration tool for Ableton Live projects. It solves the core problem music producers face: tracking changes to `.als` project files across sessions, collaborators, and machines — without losing work or stepping on each other's changes.

### The Problem It Solves

- `.als` files are opaque binary blobs — you can't diff or merge them
- Sending project files over Dropbox/Google Drive causes silent overwrites
- Email/WeTransfer versioning means you end up with `Project_v3_FINAL_useThis.acid.wav` nightmare folders
- Collaborating with someone (Chris + Steven) requires manual file coordination
- There's no history of what changed between sessions, or why

### How Clavus Works

Clavus snapshots your `.als` file every time you save in Ableton. It stores the raw `.als` bytes as a content-addressed blob, parses the project structure for display, and records a message you write. Over time, you build a timeline of project states — each labeled with what changed and why.

Collaboration happens via a persistent **relay server** running on Chris's always-on Mac, exposed through Tailscale MagicDNS. Steven connects from Windows and syncs snapshots and cues through it. No cloud service, no subscription — just an always-on machine on your tailnet.

---

## 2. Core Concepts

### Project

A **Project** is a tracked Ableton Live set. It's defined by:
- The path to its `.als` file
- A human-readable name (derived from the filename)
- A current HEAD snapshot hash
- A branch name (default: `main`)

Projects are registered in `~/.clavus/index.json`. Multiple projects can coexist; you switch between them with `clavus project <name>`.

```python
# clavus/store.py — ClavusProject dataclass
@dataclass
class ClavusProject:
    name: str
    root_als: str         # /Users/slowhands/Desktop/Drums/Drums.als
    created_at: float
    head: Optional[str]    # current snapshot hash
    branch: str = "main"
    shared: bool = False   # visible to collaborators via relay
    last_remote_head: str  # optimistic lock — relay HEAD last seen
```

### Snapshot

A **Snapshot** is a point-in-time record of a project. Think Git commit:

- **Identity:** SHA256 of the raw `.als` bytes — any Ableton save that changes the file produces a new snapshot automatically
- **Parent:** The previous snapshot hash (forms a history chain, like Git's parent refs)
- **Parsed state:** The project JSON (track list, BPM, clips, markers) stored separately for diffing
- **Message:** Your description of what changed
- **Samples:** SHA256 hashes of audio files referenced by the project, with relative paths

```python
# clavus/store.py — Snapshot dataclass
@dataclass
class Snapshot:
    hash: str              # SHA256 of raw .als bytes (identity)
    timestamp: float
    message: str           # "added reverb return"
    parent: Optional[str]  # previous snapshot hash
    project_path: str
    track_count: int
    bpm: float
    tags: list[str]
    als_hash: Optional[str]    # same as hash (raw .als backup)
    content_hash: str          # SHA256 of parsed JSON (for diff)
    sample_hashes: list[str]   # SHA256 of audio files
    sample_paths: dict[str,str] # hash → relative path from project root
```

Snapshots are stored as `.meta` files at:
```
~/.clavus/objects/<hash[0:2]>/<hash>.meta
```

### Cue

A **Cue** is a timeline-anchored comment pinned to a specific position in the Ableton arrangement (format: `bar.beat.tick`, e.g. `4.1.0`). Cues are the collaboration layer — Chris leaves notes for Steven about what to do in a section, or flags something to revisit.

```python
# clavus/cues.py — Cue dataclass
@dataclass
class Cue:
    id: str
    position: str      # "4.1.0" (bar.beat.tick)
    text: str
    author: str
    status: str        # "pending" | "done" | "wontfix"
    track_name: str    # which track this cue belongs to
    assignee: str
    in_progress: bool
    replies: list       # threaded replies
```

Cue files live at:
```
~/.clavus/cues/<project_name>/<cue_id>.json
```

### Peer

A **Peer** is any Clavus instance that can sync with another. Currently two peers are defined:

| Peer | Machine | Tailscale DNS | Role |
|------|---------|---------------|------|
| Chris | MacBook Pro | `slow-hands-studio-1` | Relay host, primary collaborator |
| Steven | Windows PC | `chrispc.tail46b8d9.ts.net` | Secondary collaborator |

Discovery works via `tailscale status --json` to enumerate peers on the tailnet.

### Relay

The **Relay** is a persistent FastAPI server running on Chris's Mac. It:

1. Receives pushed snapshots from any peer and stores them locally
2. Serves pulled snapshots/blobs to requesting peers
3. Broadcasts cue events to connected WebSocket clients (real-time updates)
4. Acts as the collaboration hub — Steven's PC reaches it via MagicDNS

**Process:** `python -m clavus.relay` or `clavus relay`  
**Local port:** `7891`  
**MagicDNS exposed:** `http://slow-hands-studio-1:7890` via `tailscale serve`

---

## 3. Project Structure

### On-Disk Layout

For a project named **"Drums"** at `~/Desktop/Drums/Drums.als`:

```
~/.clavus/                          # Clavus store root
├── index.json                     # Project registry (name → metadata)
├── config.json                     # Store version, creation date
├── remotes.json                    # Remote relay/peer configurations
├── refs/
│   ├── HEAD                        # Current HEAD hash
│   ├── _last_project               # Which project was last active
│   ├── heads/main                  # Branch HEAD hashes
│   └── tags/<tagname>              # Tag references
├── objects/                        # Content-addressed blob storage
│   ├── <hash[0:2]>/
│   │   ├── <full_hash>            # Parsed project JSON blob
│   │   ├── <full_hash>.meta       # Snapshot metadata
│   │   ├── <full_hash>.sample     # Sample filename + relative path
│   │   └── ...                    # (hash = SHA256 of raw .als or sample bytes)
│   └── ...
├── cues/
│   └── Drums/
│       ├── cue_<uuid1>.json
│       └── cue_<uuid2>.json
└── backups/
    └── clavus-auto-20250514.tar.gz

~/Desktop/Drums/
├── Drums.als                       # The actual Ableton project file
└── Samples/                        # Audio files referenced by .als
    └── Kicks/
        └── kick_01.wav
```

### Key Files

**`~/.clavus/index.json`** — Project registry
```json
{
  "Drums": {
    "name": "Drums",
    "root_als": "/Users/slowhands/Desktop/Drums/Drums.als",
    "head": "a3f72b8c...",
    "branch": "main",
    "shared": true,
    "last_remote_head": "e9a40c..."
  },
  "_last_project": "Drums"
}
```

**`~/.config/clavus/config.json`** — User configuration
```json
{
  "author": "chris",
  "port": 7890,
  "host": "0.0.0.0",
  "default_project": "Drums",
  "projects_dir": "~/Clavus/Projects"
}
```

---

## 4. Relay Architecture

### How the Relay Works

The relay is a **FastAPI application** (`clavus/web.py`) running on port `7891` locally. Tailscale serve proxies it to the tailnet at port `7890`, making it reachable via MagicDNS.

```
Steven's Windows PC
        │
        ▼
http://chrispc.tail46b8d9.ts.net:7890  (Tailscale MagicDNS)
        │
        ▼
tailscale serve (proxy on Chris's Mac)
        │
        ▼
localhost:7891  (FastAPI relay process)
```

### Starting the Relay

```bash
# Option 1: via module
python -m clavus.relay

# Option 2: via CLI
clavus relay

# Option 3: as a shared project relay (scoped to specific projects)
clavus relay --projects Drums "Bass Project"
```

The relay is persistent — it runs indefinitely as a background server.

### API Endpoints

| Endpoint | Method | Purpose |
|----------|--------|---------|
| `/api/ping` | GET | Health check |
| `/api/projects` | GET | List shared projects |
| `/api/project?name=<name>` | GET | Get project state + snapshot history |
| `/api/cues?project=<name>` | GET | List all cues |
| `/api/cues` | POST | Create a new cue |
| `/api/snapshots/<hash>` | GET | Get snapshot detail |
| `/api/sync/pull?name=<name>` | GET | Pull snapshot + cue history |
| `/api/sync/push-snapshots` | POST | Push snapshot metadata |
| `/api/sync/check-blobs` | POST | Ask relay which blobs we need |
| `/api/sync/push-blobs` | POST | Upload blob data |
| `/api/blobs/<hash>` | GET | Download a blob |
| `/api/stems/manifest/<project>` | POST | Push stem manifest |
| `/ws?project=<name>` | WebSocket | Real-time cue event broadcast |

### Conflict Detection on the Relay

The relay uses an **optimistic locking** strategy to prevent silent overwrites:

1. When a peer pushes snapshots, it sends `expected_parent` — the hash it believes is the relay's current HEAD
2. If `expected_parent != relay's current HEAD` → relay returns **HTTP 409 Conflict** with the actual relay HEAD
3. The peer can then pull first, or force-push with `--force`
4. The relay also maintains a per-project threading lock to serialize concurrent pushes

```python
# clavus/web.py — conflict check
if not body.force and body.expected_parent is not None:
    current_head = proj.head
    if current_head and current_head != body.expected_parent:
        raise HTTPException(409, detail={
            "error": "head divergence — pull first",
            "relay_head": current_head,
        })
```

### Push/Pull Flow Through Relay

**Push** (`clavus push`):
```
Peer                               Relay
  │                                   │
  │  POST /api/sync/push-snapshots    │
  │  { snapshots: [...],              │
  │    expected_parent: "abc123" }    │
  │ ──────────────────────────────►   │
  │                                   │
  │  ← 409 (conflict) or 200 OK       │
  │                                   │
  │  POST /api/sync/check-blobs       │
  │  { hashes: [...] }                │
  │ ──────────────────────────────►   │
  │                                   │
  │  ← { missing: ["hash1", "hash2"] }│
  │                                   │
  │  POST /api/sync/push-blobs        │
  │  [ { hash, data: b64 }, ... ]     │
  │ ──────────────────────────────►   │
  │                                   │
  │  POST /api/sync/push              │
  │  { cues: [...] }                  │
  │ ──────────────────────────────►   │
  │                                   │
  │  ← 200 OK                         │
```

**Pull** (`clavus pull`):
```
Peer                               Relay
  │                                   │
  │  GET /api/sync/pull?name=Drums    │
  │ ──────────────────────────────►   │
  │                                   │
  │  ← { snapshots: [...],            │
  │      cues: [...],                 │
  │      project: {...} }             │
  │                                   │
  │  POST /api/sync/check-blobs       │
  │  (with all blob hashes from history)
  │ ──────────────────────────────►   │
  │                                   │
  │  ← { missing: ["hash3", ...] }    │
  │                                   │
  │  GET /api/blobs/<hash3>           │
  │  (for each missing blob)          │
  │ ──────────────────────────────►   │
```

### Discovery

Discovery of peers uses Tailscale's own API:

```python
# clavus/discovery.py — uses tailscale status --json
result = subprocess.run(
    ["tailscale", "status", "--json"],
    capture_output=True, text=True, timeout=5,
)
data = json.loads(result.stdout)
# Peers are enumerated from the Tailscale netmap
```

For the relay, Chris's Mac is the always-on host — Steven just needs to know the MagicDNS name (`slow-hands-studio-1`) and the relay URL is pre-configured.

---

## 5. Sync Protocol

### How Two Peers Sync

Clavus supports two sync modes:

1. **Relay-mediated** (`clavus push` / `clavus pull`) — all traffic through the persistent relay server
2. **Direct P2P** (`clavus p2p --connect <peer>`) — raw TCP socket, machine-to-machine

Both modes use the same conflict detection logic.

### Conflict Detection (Hash-Based)

Clavus uses a Git-style **expected_parent** check to prevent silent overwrites:

```
When P1 (Steven) wants to push to P2 (Chris via relay):

P1 sends:
  expected_parent = "abc123"  # what P1 thinks P2's HEAD is
  snapshots = [new snapshots since abc123]

P2 checks:
  if current_HEAD != expected_parent:
      return HTTP 409 Conflict { relay_head: current_HEAD }
  # else: accept the push
```

The `last_remote_head` field in `ClavusProject` tracks what each peer last successfully synced to, enabling the conflict detection on subsequent syncs.

**P2P mode** uses the same logic over raw TCP frames:

```
Listener (Chris)                 Connector (Steven)
     │                                  │
     │  MANIFEST {                      │
     │    expected_head: "abc123" } ──►│
     │                                  │
     │  [checks: our HEAD == abc123?]   │
     │  ← CONFLICT { head, message }    │
     │     (if mismatch)                │
     │                                  │
     │  ← MANIFEST { head }             │
     │                                  │
     │  [sync blobs]                    │
```

### What Gets Transferred

During a full sync (push + blobs), these assets are transferred:

| Asset | Description | How Identified |
|-------|-------------|----------------|
| Snapshot metadata | `.meta` JSON files | SHA256 of raw .als bytes |
| Parsed project JSON | Content blob for diff/comparison | SHA256 of serialized JSON |
| Raw `.als` backup | Byte-for-byte copy of the Ableton file | SHA256 of `.als` bytes |
| Audio samples | Audio files referenced in the project | SHA256 of file bytes |
| Cues | Timeline-anchored comments | UUID-based JSON files |

**Snapshots are deduplicated by content hash.** If 10 snapshots all reference the same unchanged sample file, that sample blob is stored once and referenced by all 10 snapshots.

### Blob Transfer Details

```python
# Phase 1: Check which blobs are missing (one batched call)
POST /api/sync/check-blobs
{ "hashes": ["content_hash1", "als_hash1", "sample_hash1", ...] }
→ { "missing": ["sample_hash1", ...] }

# Phase 2: Upload missing blobs in parallel batches
POST /api/sync/push-blobs
[
  { "hash": "sha256...", "data": "base64..." },
  ...
]
→ 200 OK
```

Sample metadata (original filename + relative path from project root) is sent alongside:
```python
POST /api/sync/sample-names
[
  { "hash": "sha256...", "name": "kick_01.wav", "relpath": "Samples/Kicks/kick_01.wav" }
]
```

---

## 6. Collaboration Workflow

### The Two Collaboration Modes

**Sequential (turn-taking):**
Chris works on the project for a week, building out the arrangement. He snapshots regularly with messages like `clavus snapshot "added 4-bar riser build"` or `clavus snapshot "parallel compression on drums"`. When he's done for the week, he pushes to the relay.

Steven pulls, opens the project in Ableton, and continues. He doesn't touch anything Chris was working on (or if he does, he snapshots first so there's a recovery point).

**Simultaneous (separate branches):**
Chris and Steven work on different aspects — Chris does arrangement/structure while Steven handles sound design on a specific track. They use Clavus branches (`clavus branch sound-design`) to isolate work, then merge when ready.

```bash
# Steven: create a branch for his work
clavus branch sound-design
clavus checkout sound-design

# Work on drums, snapshot
clavus snapshot "replaced stock kicks with analog"

# Merge back when done
clavus checkout main
clavus merge sound-design
```

### Day-to-Day With Chris and Steven

**Chris's workflow (relay host, primary collaborator):**

```bash
# Start the relay (run once on machine boot)
clavus relay

# Open the TUI to manage cues + snapshots
clavus tui

# Or CLI-driven workflow:
clavus project Drums
clavus snapshot "tightened snare sidechain"
clavus push

# Check sync status
clavus doctor
```

**Steven's workflow (Windows PC):**

```bash
# Pull latest changes
clavus pull

# Open Ableton, do work, save

# Snapshot + push
clavus snapshot "darkened the reverb tail"
clavus push
```

### Role of Snapshots in Recovery

If Steven accidentally breaks something Chris sent:

```bash
# See history
clavus log

# See what changed in a specific snapshot
clavus diff abc1234

# Restore a previous snapshot (rewrites .als file + re-materializes samples)
clavus restore abc1234

# Or just look at what it was without restoring
clavus checkout abc1234  # switches HEAD ref without touching the .als file
```

### Resolving Conflicts

**Scenario:** Chris pushes changes, then Steven pushes without pulling first.

```
Chris pushes → relay HEAD = abc123 (Steven hasn't pulled yet)
Steven pushes → expected_parent = "" (nothing local)
             → relay accepts, HEAD = def456
             → Steven's push succeeds (no conflict because empty expected_parent)
```

**Scenario:** Steven pulls, works on the arrangement, then Chris pushes to the same project.

```
Steven pulls → his local HEAD = abc123
Chris pushes → relay HEAD = def456
Steven pushes → expected_parent = abc123
              → relay checks: current_HEAD (def456) != expected_parent (abc123)
              → 409 CONFLICT returned with relay_head = def456
              → Steven must `clavus pull` first
```

When a 409 Conflict is received, Clavus:
1. Displays the error with the conflicting relay HEAD hash
2. Auto-updates `last_remote_head` on the remote config so subsequent pulls work cleanly
3. Suggests `clavus pull` to fetch the divergent state

---

## 7. Clavus + CRUX

### Repository Relationship

```
/Users/slowhands/Developer/clavus/      ← Clavus project (this repo)
/Users/slowhands/Developer/crux-tui/    ← CRUX TUI library (imported)
```

**Clavus imports CRUX as its TUI component.** Clavus does not maintain its own terminal UI — instead, it depends on the `crux-tui` library which provides the Textual-based sample curation interface. Think of it as Clavus using a shared component rather than reinventing the wheel.

```python
# In Clavus TUI, CRUX components are imported and integrated
# The CRUX palette (PALETTE) is used for color consistency
# /Users/slowhands/Developer/crux-tui/crux.py defines the full CRUX app
```

### How CRUX Is Used in Clavus

Looking at the Clavus TUI (`clavus/tui.py`), the CRUX import relationship is primarily through:

1. **Shared color palette** — Clavus TUI defines its own `C` color map but it mirrors CRUX's design language (dark theme, teal accent)
2. **Conceptual lineage** — both tools target music producers who live in the terminal

The actual CRUX library (`crux.py`) is a standalone sample curation tool. Clavus's TUI is purpose-built for cue management and snapshot browsing.

### Design/Beautify Branch

Both repositories have a `design/beautify` branch with a new **Studio Suite theme** — a visual refresh with different accent colors and layout improvements. This branch represents a post-v0.1.0 visual overhaul and is not yet merged to main.

---

## 8. Design Decisions

### Why Snapshots Over Full File Sync

Ableton `.als` files are monolithic — saving one creates a brand new file with a new SHA256. You can't merge them, and tracking every byte-level change would explode storage.

**Snapshots solve this by:**
1. **Hash = content identity** — Two identical `.als` files produce the same snapshot hash, so duplicates are free
2. **Content-addressed storage** — Any blob (`.als` or sample) is stored once and referenced by hash. 10 snapshots of the same project = 1 `.als` blob + 10 tiny `.meta` files
3. **Parsed state for diff** — The raw `.als` bytes are preserved, but the parsed JSON is stored separately so Clavus can tell you "you added 3 clips and changed BPM from 120 to 124" without re-parsing every time
4. **History traversal** — The parent chain means you can walk backwards through any point in time

### Why Relay Over Pure P2P

**Pure P2P problems that a relay solves:**

1. **Addressability** — To connect P2P, you need to know the other's IP/port. On a LAN behind NAT, this is fragile. Tailscale MagicDNS solves addressability, but a relay still provides a stable rendezvous point.
2. **Availability** — If Chris's machine is asleep, Steven can't P2P-connect to pull. The relay can be always-on on Chris's server-grade machine.
3. **Bandwidth asymmetry** — One peer might be on a fast connection, the other on a slow mobile link. A relay can batch and optimize transfers.
4. ** NAT traversal** — Corporate/WiFi networks block incoming connections. A persistent outbound connection to a relay avoids this.

**Pure P2P advantage Clavus still supports:**
For two always-on machines on the same fast tailnet, direct P2P (`clavus p2p --connect peer.tail46b8d9.ts.net`) is available and avoids the relay altogether.

### Why Tailscale MagicDNS

The alternatives considered:

| Approach | Problem |
|----------|---------|
| Static IP + port forwarding | Loses NAT, requires router config, security risk |
| ngrok / cloudflare tunnel | External dependency, relay traffic goes through third party |
| mDNS / Bonjour | Doesn't cross the internet, only works on LAN |
| Static IP in hosts file | Requires knowing the IP, IP changes on reconnection |

**Tailscale MagicDNS** gives each machine a stable, memorable DNS name (`slow-hands-studio-1`, `chrispc.tail46b8d9.ts.net`) that resolves to the machine regardless of IP. Traffic stays on the Tailscale encrypted tailnet — no exposure to the public internet. It's zero-config after the initial Tailscale install.

---

## 9. CLI Commands

### Project Management

```bash
# Initialize a new project (finds .als file, creates initial snapshot, git init)
clavus init [path]              # path to .als file or directory containing one

# List all tracked projects
clavus projects

# Switch active project
clavus project <name>

# Share a project (visible to collaborators via relay)
clavus project <name> --share

# Make a project private
clavus project <name> --private

# Toggle sharing
clavus project <name> --share/--private
```

### Snapshots

```bash
# Create a snapshot of the current project state
clavus snapshot "message" [--tag tag1,tag2]

# Allow snapshots even with frozen tracks (cross-platform crash risk)
clavus snapshot "message" --allow-frozen

# Show snapshot history (linear)
clavus log

# Show snapshot history (branch graph)
clavus log --graph

# Show what changed in a snapshot (vs its parent)
clavus diff [hash]

# Visual timeline diff
clavus diff --visual [hash]

# Restore project to a specific snapshot
clavus restore <hash>

# Checkout a snapshot (update HEAD ref, don't touch .als file)
clavus checkout <hash>

# List / create branches
clavus branch            # list
clavus branch <name>     # create

# Switch branches (clavus + git)
clavus checkout <branch>

# Merge branches
clavus merge <branch>
```

### Cues

```bash
# Add a timeline-anchored cue
clavus cue "check the sidechain on this" @4.1.0

# List all cues
clavus cues

# List only pending (not done) cues
clavus cues --pending

# Resolve/dismiss a cue
clavus cue resolve <cue_id>

# Assign a cue
clavus cue assign <cue_id> steven
```

### Status

```bash
# Show current project state
clavus status

# Full health check (index, backups, relay, network, Tailscale)
clavus doctor
```

### Sync

```bash
# Push to all configured remotes
clavus push

# Pull from all configured remotes
clavus pull

# Start auto-sync daemon (watches for file changes and pushes)
clavus sync

# Add a remote
clavus remote add steven http://chrispc.tail46b8d9.ts.net:7890

# List remotes
clavus remote

# Remove a remote
clavus remote remove <name>
```

### P2P (Direct Connection)

```bash
# Discover peers on the tailnet
clavus p2p

# Start listening for incoming P2P connections
clavus p2p --host

# Connect directly to a peer by DNS name
clavus p2p --connect chrispc.tail46b8d9.ts.net
```

### Relay Server

```bash
# Start the relay server (persistent)
clavus relay

# Start relay scoped to specific projects
clavus relay --projects Drums "Bass Project"

# Tailscale serve (exposes relay at port 7890 via MagicDNS)
tailscale serve --bg --http 7890 http://localhost:7890
```

### TUI

```bash
# Open the terminal UI
clavus tui
```

### Recovery

```bash
# Repair/recover from corrupted index
clavus repair

# Force re-scan from scratch
clavus repair --force

# Set .als path during recovery
clavus repair --set-als 'ProjectName=/path/to/project.als'
clavus repair --set-als 'all=/path/to/ProjectDir/'

# Restore entire store from backup
clavus restore-store backups/clavus-20250514_120000.tar.gz

# Create full store backup
clavus backup
```

---

## 10. Future Considerations

### Session Notes ✅ IMPLEMENTED

Longer-form notes attached to a snapshot. Use `--notes` at snapshot time, or use the `clavus note` command to read/write/append notes after the fact.

```
clavus snapshot "expanded arrangement" --notes "Added 8-bar breakdown.
The transition at bar 48 is still rough — needs a riser.
Steven should look at the reverb return level on the snare."

clavus note read              # show notes for current HEAD
clavus note write <text>      # replace notes on a snapshot
clavus note append <text>     # add to existing notes
clavus note write -f note.md  # write from file (good for long notes)
```

Notes are stored in the snapshot `.meta` JSON file alongside the snapshot metadata. They support markdown and are displayed in `clavus log --verbose` output.

### Audio Preview Attachments

Currently, cues are text-only. A natural extension is allowing audio preview attachments — a short `.wav` recording (e.g., a phone recording of a reference track, or a bounced stem) attached to a cue so Steven knows exactly what Chris heard in his head.

Implementation would likely:
- Store the audio as a blob in `objects/`
- Reference it from the cue JSON via hash
- Attach a `preview_hash` field to `Cue`

### GUI

The TUI (`clavus tui`) handles cue management and snapshot browsing, but a graphical client could offer:

- Visual arrangement view showing clip positions and cue markers
- Waveform previews of samples referenced in snapshots
- Drag-and-drop branch management
- Real-time collaboration presence (see when the other person is working)
- A macOS menu-bar app for quick snapshot/push without opening a terminal

The `design/beautify` branch on both repos suggests the Studio Suite theme is the intended visual direction for any GUI work.

### Visual Diff Timeline

Currently `clavus diff --visual` renders a CLI-based arrangement view. A future enhancement could generate an actual visual timeline — a side-by-side track view showing what was added, removed, or moved between two snapshots, with playback position markers.

---

## File Index

| File | Purpose |
|------|---------|
| `clavus/cli.py` | All CLI commands + argument parsing |
| `clavus/tui.py` | Textual-based TUI application |
| `clavus/web.py` | FastAPI relay server (all `/api/*` endpoints) |
| `clavus/store.py` | `BlobStore`, `ClavusProject`, `Snapshot`, content-addressed storage |
| `clavus/sync.py` | Push/pull client, sync daemon, blob transfer |
| `clavus/p2p_transport.py` | Raw TCP P2P transport, frame protocol |
| `clavus/cues.py` | `CueStore`, cue CRUD, WebSocket broadcast |
| `clavus/parser.py` | `.als` file parsing (XML → Python objects) |
| `clavus/discovery.py` | Tailscale peer discovery |
| `clavus/config.py` | User config (`~/.config/clavus/config.json`) |
| `clavus/watch.py` | File system watcher for auto-snapshot |
| `clavus/visual_diff.py` | CLI arrangement diff renderer |
| `clavus/progress.py` | Spinners + progress feedback |
| `clavus/helpers.py` | Path utilities, store/project resolution |
| `crux.py` (CRUX) | Sample curation TUI library |
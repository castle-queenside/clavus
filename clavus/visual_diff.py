"""Clavus — visual diff renderer for Ableton project snapshots.

Transforms ProjectDiff into ASCII/Unicode timeline visualizations
that show how the arrangement changed between snapshots.

Two output modes:
  1. Terminal (ANSI colors, Unicode box drawing) — for CLI and TUI
  2. HTML/SVG — for the web companion

The renderer shows actual clip positions from the parser's clip extraction,
giving a proper Ableton-style arrangement view of each track.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Optional

from clavus.parser import Project, Clip
from clavus.store import ProjectDiff, TrackDiff


# ─── Color constants (CRUX theme — same as TUI) ────────────────────────

C = {
    "bg": "#0b1418",
    "surface": "#0f1a20",
    "border": "#1a3040",
    "accent": "#1a9e9e",
    "fg": "#b8c8c8",
    "dim": "#6a8a8a",
    "muted": "#3a5a65",
    "yellow": "#d4a030",
    "green": "#44cc44",
    "red": "#ff4444",
    "orange": "#ff8844",
    "blue": "#4488ff",
}

# ANSI terminal color codes
ANSI = {
    "accent": "\033[36m",
    "green": "\033[32m",
    "red": "\033[31m",
    "yellow": "\033[33m",
    "dim": "\033[2m\033[37m",
    "bold": "\033[1m",
    "reset": "\033[0m",
    "orange": "\033[38;5;208m",
    "blue": "\033[34m",
    "cyan": "\033[36m",
    "magenta": "\033[35m",
}

# ─── Clip Color Palette (Ableton-like) ────────────────────────────────
# Maps Ableton color indices to ANSI colors for clip rendering
# HTML hex color palette (same as web companion)
ABLE_COLOR_HEX = [
    "#e8e8e8", "#cc5050", "#e88c50", "#e8c850", "#50cc50",
    "#50cc8c", "#50cccc", "#508ce8", "#8c50e8", "#cc50cc",
    "#cc508c", "#e8a060", "#a0e860", "#60e8a0", "#60e8e8",
    "#a060e8", "#e860cc", "#e860a0", "#404040", "#808080",
    "#b0b0b0", "#d08040", "#40d080", "#6080d0",
]

def _ableton_hex(color_idx: int) -> str:
    """Map Ableton color index (0-23) to a visible hex color."""
    if 0 <= color_idx < len(ABLE_COLOR_HEX):
        return ABLE_COLOR_HEX[color_idx]
    return "#e8a060"  # Fallback orange

ABLE_COLORS = {
    1: ANSI["blue"],
    2: ANSI["cyan"],
    3: ANSI["green"],
    5: ANSI["yellow"],
    6: ANSI["orange"],
    7: ANSI["red"],
    8: ANSI["magenta"],
    11: ANSI["dim"],  # Default grey
    12: ANSI["accent"],
    24: ANSI["cyan"],
}
ABLE_COLOR_BG = {
    1: "\033[44m",      # Blue bg
    2: "\033[46m",      # Cyan bg  
    3: "\033[42m",      # Green bg
    5: "\033[43m",      # Yellow bg
    6: "\033[48;5;208m",# Orange bg
    7: "\033[41m",      # Red bg
    8: "\033[45m",      # Magenta bg
    11: "\033[48;5;236m",# Dark grey bg
    12: "\033[48;5;30m",# Teal bg
    24: "\033[48;5;25m",# Blue bg
}

TIMELINE_WIDTH = 60  # Characters wide for each timeline
TRACK_LABEL_WIDTH = 36  # Characters for track name column
CLIP_BLOCK = "█"  # Filled block for a clip
CLIP_EMPTY = "·"  # Dot for empty space


def _format_bpm(bpm: float) -> str:
    return f"{bpm:.0f}"


def _clip_color_ansi(color: int, fg: bool = True) -> str:
    """Get ANSI color code for a clip based on Ableton color index."""
    if fg:
        return ABLE_COLORS.get(color, ANSI["dim"])
    return ABLE_COLOR_BG.get(color, "\033[48;5;236m")


def _max_timeline_beats(projects: list[Project]) -> float:
    """Find the maximum end position across all clips in a list of projects."""
    max_beats = 0.0
    for proj in projects:
        if proj is None:
            continue
        for t in proj.tracks:
            for c in t.clips:
                max_beats = max(max_beats, c.end_beats)
    return max_beats or 64.0  # Default 8 bars if no clips


# ─── Clip-Level Timeline Renderer ──────────────────────────────────


def render_timeline_bar(
    clips: list[Clip],
    width: int = TIMELINE_WIDTH,
    max_beats: float = 64.0,
    gap_start: float = 0.0,
    gap_end: float | None = None,
) -> str:
    """Render a single track's arrangement as a clip-position timeline.

    Args:
        clips: List of Clip objects for this track
        width: Character width of the timeline
        max_beats: Total timeline length in beats (the "canvas" width)
        gap_start: Beats before the timeline starts (for side-by-side offset)
        gap_end: If set, beats AFTER the timeline ends (for alignment)

    Returns:
        ANSI-colored string representing clip positions
    """
    if not clips or max_beats <= 0:
        return f"{ANSI['dim']}{CLIP_EMPTY * width}{ANSI['reset']}"

    # Build a character array for the timeline — use block chars, not spaces
    # Each character gets: color_prefix + block + reset
    timeline = [f"{ANSI['dim']}{CLIP_EMPTY}{ANSI['reset']}"] * width

    for clip in clips:
        # Map clip position to character index
        start_char = int(clip.start_beats / max_beats * width)
        end_char = int(clip.end_beats / max_beats * width)

        # Clamp to bounds
        start_char = max(0, min(start_char, width - 1))
        end_char = max(0, min(end_char, width))

        color_code = _clip_color_ansi(clip.color, fg=False)
        block_char = CLIP_BLOCK if clip.color != 11 else "░"
        for pos in range(start_char, end_char):
            if 0 <= pos < width:
                timeline[pos] = f"{color_code}{block_char}{ANSI['reset']}"

    return "".join(timeline)


# ─── Side-by-Side Visual Diff ──────────────────────────────────────


def render_side_by_side(
    before_proj: Project | None,
    after_proj: Project | None,
    before_track_diffs: list[TrackDiff],
    after_track_diffs: list[TrackDiff],
    before_markers: list[str],
    after_markers: list[str],
    before_bpm: Optional[float] = None,
    after_bpm: Optional[float] = None,
) -> str:
    """Render a side-by-side visual diff of before and after states.

    This is the main entry point for CLI `clavus diff --visual`.

    Args:
        before_proj: Full Project for the "before" state (None if no parent)
        after_proj: Full Project for the "after" state
        before_track_diffs: TrackDiff list for before
        after_track_diffs: TrackDiff list for after
        before_markers: Marker names for before
        after_markers: Marker names for after
        before_bpm: BPM of before snapshot
        after_bpm: BPM of after snapshot

    Returns:
        ANSI-colored string for terminal display
    """
    lines = []
    time_width = TIMELINE_WIDTH

    # Determine total timeline range (union of both projects' clips)
    projects = [p for p in [before_proj, after_proj] if p is not None]
    max_beats = _max_timeline_beats(projects)
    # Round up to next power-of-2 bar boundary for clean display
    bar_size = 4.0  # Assume 4/4 at default
    max_beats = max(64.0, ((max_beats + bar_size - 1) // bar_size) * bar_size)

    # Show scale markers
    beats_per_char = max_beats / time_width

    # ── Build ordered track lists ──
    before_tracks: dict[str, TrackDiff] = {}
    after_tracks: dict[str, TrackDiff] = {}
    before_clips: dict[str, list[Clip]] = {}
    after_clips: dict[str, list[Clip]] = {}
    all_names: list[str] = []

    for td in before_track_diffs:
        before_tracks[td.name] = td
        if td.name not in all_names:
            all_names.append(td.name)

    for td in after_track_diffs:
        after_tracks[td.name] = td
        if td.name not in all_names:
            all_names.append(td.name)

    # Map clip data from projects to track names
    if before_proj:
        for t in before_proj.tracks:
            before_clips[t.name] = t.clips
    if after_proj:
        for t in after_proj.tracks:
            after_clips[t.name] = t.clips

    # ── Scale ruler ──
    bpm_display = f"  {ANSI['bold']}Before{ANSI['reset']}"
    if before_bpm:
        bpm_display += f" {ANSI['dim']}{_format_bpm(before_bpm)}bpm{ANSI['reset']}"
    bpm_display2 = f"  {ANSI['bold']}After{ANSI['reset']}"
    if after_bpm:
        bpm_display2 += f" {ANSI['dim']}{_format_bpm(after_bpm)}bpm{ANSI['reset']}"

    lines.append("")
    lines.append(
        f"  {bpm_display}"
        f"{' ' * (TRACK_LABEL_WIDTH + time_width - len(bpm_display) - len(bpm_display2) + 2)}"
        f"{bpm_display2}"
    )

    # Beat scale ruler — show 16-beat marks
    scale_before = ""
    for i in range(0, int(max_beats), 16):  # 4-bar marks
        pos = int(i / max_beats * time_width)
        while len(scale_before) < pos:
            scale_before += " "
        scale_before += "│"
    scale_before = scale_before.ljust(time_width)
    
    lines.append(
        f"  {'Beat':<{TRACK_LABEL_WIDTH}}{ANSI['dim']}{scale_before}{ANSI['reset']}  {ANSI['dim']}{scale_before}{ANSI['reset']}"
    )

    divider = "  " + "─" * (TRACK_LABEL_WIDTH + time_width) + "  " + "─" * time_width
    lines.append(divider)

    # ── Track rows ──
    for name in all_names:
        bt = before_tracks.get(name)
        at = after_tracks.get(name)

        # Determine row annotation
        annotation = ""
        detail_parts = []
        if bt and not at:
            annotation = f" {ANSI['red']}◄ removed{ANSI['reset']}"
        elif at and not bt:
            annotation = f" {ANSI['green']}◄ ▲ added{ANSI['reset']}"
        elif bt and at and bt.status != "unchanged":
            if at.devices_added:
                detail_parts.append(f"+{','.join(d[:10] for d in at.devices_added)}")
            if at.devices_removed:
                detail_parts.append(f"-{','.join(d[:10] for d in at.devices_removed)}")
            if at.frozen_changed is not None:
                detail_parts.append("frozen" if at.frozen_changed else "unfrozen")
            if at.mute_changed is not None:
                detail_parts.append("muted" if at.mute_changed else "unmuted")
            if detail_parts:
                annotation = f" {ANSI['yellow']}◄ {', '.join(detail_parts)}{ANSI['reset']}"

        # Clip-count change indicator
        if bt and at:
            before_clip_count = len(before_clips.get(name, []))
            after_clip_count = len(after_clips.get(name, []))
            if before_clip_count != after_clip_count:
                if not detail_parts:
                    diff_count = after_clip_count - before_clip_count
                    if diff_count > 0:
                        annotation = f" {ANSI['accent']}◄ +{diff_count} clips{ANSI['reset']}"
                    else:
                        annotation = f" {ANSI['dim']}◄ {diff_count} clips{ANSI['reset']}"

        # Left side (before)
        left_label = f"{name:<{TRACK_LABEL_WIDTH}}"
        if bt:
            left_clips = before_clips.get(name, [])
            left_bar = render_timeline_bar(
                left_clips, time_width, max_beats=max_beats,
            )
        else:
            left_bar = f"{ANSI['dim']}{CLIP_EMPTY * time_width}{ANSI['reset']}"

        # Right side (after)
        if at:
            right_clips = after_clips.get(name, [])
            right_bar = render_timeline_bar(
                right_clips, time_width, max_beats=max_beats,
            )
        else:
            right_bar = f"{ANSI['dim']}{CLIP_EMPTY * time_width}{ANSI['reset']}"

        lines.append(f"  {left_label}{left_bar}  {right_bar}{annotation}")

    # ── Marker rows ──
    new_markers = set(after_markers) - set(before_markers)
    marker_tags_before = " ".join(
        f"{ANSI['dim']}[{m[:12]}]{ANSI['reset']}"
        for m in before_markers[:6]
    ) if before_markers else f"{ANSI['dim']}—{ANSI['reset']}"
    
    marker_tags_after = " ".join(
        f"{ANSI['accent'] if m in new_markers else ANSI['dim']}[{m[:12]}]{ANSI['reset']}"
        for m in after_markers[:6]
    ) if after_markers else f"{ANSI['dim']}—{ANSI['reset']}"

    lines.append(
        f"  {'Markers':<{TRACK_LABEL_WIDTH}}{marker_tags_before}"
        f"  {marker_tags_after}"
    )

    # ── BPM change indicator ──
    if before_bpm and after_bpm and before_bpm != after_bpm:
        bpm_diff = after_bpm - before_bpm
        direction = "▲ faster" if bpm_diff > 0 else "▼ slower"
        lines.append("")
        lines.append(
            f"  {ANSI['yellow']}● Tempo: {before_bpm:.0f} → {after_bpm:.0f} bpm "
            f"({direction} by {abs(bpm_diff):.0f}){ANSI['reset']}"
        )

    # ── Summary footer ──
    added_count = len([t for t in all_names if after_tracks.get(t) and not before_tracks.get(t)])
    removed_count = len([t for t in all_names if before_tracks.get(t) and not after_tracks.get(t)])
    modified_count = len([t for t in all_names
                         if before_tracks.get(t) and after_tracks.get(t)
                         and before_tracks[t].status != "unchanged"])

    # Clip change summary
    clip_diffs = []
    for name in all_names:
        bt = before_tracks.get(name)
        at = after_tracks.get(name)
        if bt and at:
            bc = len(before_clips.get(name, []))
            ac = len(after_clips.get(name, []))
            if bc != ac:
                clip_diffs.append((name, ac - bc))

    summary_parts = []
    if added_count:
        summary_parts.append(f"{ANSI['green']}+{added_count} tracks{ANSI['reset']}")
    if removed_count:
        summary_parts.append(f"{ANSI['red']}-{removed_count} tracks{ANSI['reset']}")
    if modified_count:
        summary_parts.append(f"{ANSI['yellow']}~{modified_count} modified{ANSI['reset']}")
    if new_markers:
        summary_parts.append(f"{ANSI['accent']}+{len(new_markers)} markers{ANSI['reset']}")
    for name, diff_count in clip_diffs[:3]:
        if diff_count > 0:
            summary_parts.append(f"{ANSI['accent']}{name}: +{diff_count} clips{ANSI['reset']}")
        else:
            summary_parts.append(f"{ANSI['dim']}{name}: {diff_count} clips{ANSI['reset']}")
    if len(clip_diffs) > 3:
        summary_parts.append(f"{ANSI['dim']}... ({len(clip_diffs) - 3} more){ANSI['reset']}")

    if summary_parts:
        lines.append("")
        lines.append(f"  {'':<{TRACK_LABEL_WIDTH}}{' | '.join(summary_parts)}")

    lines.append("")
    return "\n".join(lines)


def render_diff_cli(diff: ProjectDiff, before_proj: Project | None = None, after_proj: Project | None = None) -> str:
    """Render a ProjectDiff as a visual timeline for the terminal/CLI.

    This is called by `clavus diff --visual`.

    Args:
        diff: The structured diff between two snapshots
        before_proj: Full project data for the "before" snapshot (if available)
        after_proj: Full project data for the "after" snapshot (if available)

    Returns:
        ANSI-colored visual diff string for terminal output
    """
    return render_side_by_side(
        before_proj=before_proj,
        after_proj=after_proj,
        before_track_diffs=diff.tracks,
        after_track_diffs=diff.tracks,
        before_markers=diff.markers_removed,
        after_markers=diff.markers_added,
        before_bpm=diff.bpm_changed[0] if diff.bpm_changed else None,
        after_bpm=diff.bpm_changed[1] if diff.bpm_changed else None,
    )


# ─── Web HTML Renderer ────────────────────────────────────────────────


def render_diff_html(diff: ProjectDiff, before_proj: Project | None = None, after_proj: Project | None = None) -> str:
    """Render a ProjectDiff as HTML for the web companion.

    Returns an HTML string with inline styles matching the CRUX theme.
    """
    added = [t for t in diff.tracks if t.status == "added"]
    removed = [t for t in diff.tracks if t.status == "removed"]
    modified = [t for t in diff.tracks if t.status == "modified"]
    unchanged = [t for t in diff.tracks if t.status == "unchanged"]

    # ── Summary stats bar ──
    total_clips_before = sum(t.clips_before for t in diff.tracks) if before_proj else 0
    total_clips_after = sum(t.clips_after for t in diff.tracks) if after_proj else 0
    clip_diff = total_clips_after - total_clips_before

    stats = []
    if added:
        stats.append(f'<span style="color:#44cc44;font-weight:bold">+{len(added)}</span>')
        stats.append(f'<span style="color:#6a8a8a;font-size:0.85em">track{"s" if len(added)!=1 else ""}</span>')
    if removed:
        stats.append(f'<span style="color:#ff4444;font-weight:bold">-{len(removed)}</span>')
        stats.append(f'<span style="color:#6a8a8a;font-size:0.85em">track{"s" if len(removed)!=1 else ""}</span>')
    if modified:
        stats.append(f'<span style="color:#d4a030;font-weight:bold">~{len(modified)}</span>')
        stats.append(f'<span style="color:#6a8a8a;font-size:0.85em">modified</span>')
    if clip_diff != 0:
        c = "#44cc44" if clip_diff > 0 else "#ff4444"
        s = "△+" if clip_diff > 0 else "▽"
        stats.append(f'<span style="color:{c};font-weight:bold">{s}{abs(clip_diff)}</span>')
        stats.append(f'<span style="color:#6a8a8a;font-size:0.85em">clips</span>')
    if diff.bpm_changed:
        b, a = diff.bpm_changed
        d = a - b
        c = "#44cc44" if d > 0 else "#ff4444"
        s = "▲" if d > 0 else "▼"
        stats.append(f'<span style="color:{c}">{s}{abs(d):.0f}</span>')
        stats.append(f'<span style="color:#6a8a8a;font-size:0.85em">bpm</span>')
    if diff.markers_added or diff.markers_removed:
        ms = []
        if diff.markers_added:
            ms.append(f'<span style="color:#44cc44">+{len(diff.markers_added)} marker{"s" if len(diff.markers_added)!=1 else ""}</span>')
        if diff.markers_removed:
            ms.append(f'<span style="color:#ff4444">-{len(diff.markers_removed)} marker{"s" if len(diff.markers_removed)!=1 else ""}</span>')
        stats.append(f'<span style="color:#6a8a8a;font-size:0.85em">{", ".join(ms)}</span>')

    summary_bar = ""
    if stats:
        summary_bar = '<div style="display:flex;gap:4px;align-items:baseline;flex-wrap:wrap;margin-bottom:10px">' + \
            ' '.join(stats) + '</div>'

    # Build a lookup for track colors from the "after" project
    track_colors: dict[str, int] = {}
    if after_proj:
        for t in after_proj.tracks:
            track_colors[t.name] = t.color

    # ── Track rows ──
    rows = []
    for td in diff.tracks:
        if td.status == "unchanged":
            continue  # Skip unchanged tracks in the compact view

        status_color = "#6a8a8a"
        status_symbol = " "
        if td.status == "added":
            status_color = "#44cc44"
            status_symbol = "+"
        elif td.status == "removed":
            status_color = "#ff4444"
            status_symbol = "-"
        elif td.status == "modified":
            status_color = "#d4a030"
            status_symbol = "~"

        # Track color bar
        tc = track_colors.get(td.name, 11)
        bar_color = _ableton_hex(tc)

        # Status dot
        dot = f'<span style="color:{status_color};font-size:16px;line-height:1">●</span>'

        # Change details (compact)
        details = ""
        if td.status == "modified":
            parts = []
            if td.devices_added:
                short_devices = [d[:20] for d in td.devices_added[:3]]
                parts.append(f"+{','.join(short_devices)}")
            if td.devices_removed:
                short_devices = [d[:20] for d in td.devices_removed[:3]]
                parts.append(f"-{','.join(short_devices)}")
            if td.frozen_changed is not None:
                parts.append("frozen" if td.frozen_changed else "unfrozen")
            if td.mute_changed is not None:
                parts.append("muted" if td.mute_changed else "unmuted")
            if parts:
                details = f'<span style="color:{status_color};font-size:0.8em;opacity:0.8;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;display:block;max-width:100%"> {", ".join(parts)}</span>'

        # Clip diff badge
        clip_badge = ""
        if td.clips_changed and (td.clips_before != td.clips_after):
            cd = td.clips_after - td.clips_before
            if cd > 0:
                clip_badge = f'<span style="color:#44cc44;font-size:0.75em;margin-left:6px">△+{cd}</span>'
            else:
                clip_badge = f'<span style="color:#ff4444;font-size:0.75em;margin-left:6px">▽{cd}</span>'

        rows.append(
            f'<tr>'
            f'<td style="width:16px;padding:4px 0 4px 4px;vertical-align:middle">{dot}</td>'
            f'<td style="padding:4px 4px;vertical-align:middle;border-left:3px solid {bar_color}">'
            f'<span style="color:#b8c8c8">{td.name}</span>{clip_badge}'
            f'</td>'
            f'<td style="padding:4px 8px;vertical-align:middle;text-align:right;overflow:hidden">'
            f'<span style="color:{status_color};font-size:0.8em;text-transform:uppercase;letter-spacing:0.5px">{td.status}</span>'
            f'{details}'
            f'</td>'
            f'</tr>'
        )

    # ── BPM change inline ──
    bpm_inline = ""
    if diff.bpm_changed:
        b, a = diff.bpm_changed
        if b != a:
            d = a - b
            arrow = "→"
            c = "#44cc44" if d > 0 else "#ff4444"
            bpm_inline = (
                f'<div style="margin-top:8px;padding-top:6px;border-top:1px solid #1a3040;'
                f'display:flex;gap:6px;align-items:center">'
                f'<span style="color:#6a8a8a;font-size:0.85em">⏱ Tempo</span>'
                f'<span style="color:#b8c8c8">{b:.0f}</span>'
                f'<span style="color:#6a8a8a">{arrow}</span>'
                f'<span style="color:#b8c8c8">{a:.0f} bpm</span>'
                f'<span style="color:{c};font-size:0.8em">({"▲" if d>0 else "▼"}{abs(d):.0f})</span>'
                f'</div>'
            )

    # ── Markers inline ──
    markers_inline = ""
    if diff.markers_added or diff.markers_removed:
        ms = []
        for m in diff.markers_added:
            ms.append(f'<span style="color:#44cc44;font-size:0.85em">+{m}</span>')
        for m in diff.markers_removed:
            ms.append(f'<span style="color:#ff4444;font-size:0.85em">-{m}</span>')
        markers_inline = (
            f'<div style="margin-top:6px;display:flex;gap:4px;flex-wrap:wrap;align-items:center">'
            f'<span style="color:#6a8a8a;font-size:0.85em">📍 Markers</span>'
            f'{", ".join(ms)}'
            f'</div>'
        )

    # ── Unchanged count footnote ──
    unchanged_note = ""
    if unchanged:
        unchanged_note = (
            f'<div style="margin-top:8px;color:#3a5a65;font-size:0.8em;border-top:1px solid #152028;padding-top:6px">'
            f'… {len(unchanged)} track{"s" if len(unchanged)!=1 else ""} unchanged'
            f'</div>'
        )

    return f"""
    <div style="background:#0b1418;border:1px solid #1a3040;border-radius:6px;padding:10px 12px;font-family:monospace;font-size:13px;animation:clavus-fade-in 150ms ease-out;overflow-x:auto;max-width:100%">
      {summary_bar}
      <table style="border-collapse:collapse;width:100%;table-layout:fixed">
        <colgroup>
          <col style="width:16px">
          <col style="width:auto;min-width:80px">
          <col style="width:auto;max-width:50%">
        </colgroup>
        <tbody>
          {''.join(rows)}
        </tbody>
      </table>
      {bpm_inline}
      {markers_inline}
      {unchanged_note}
    </div>
    """

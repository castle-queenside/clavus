"""CLI progress utilities — spinner, progress bar, status helpers.

Provides animated feedback for long-running CLI operations.
- Spinner: indeterminate progress (connecting, scanning, searching)
- ProgressBar: determinate progress with % and ETA (push, pull, backup)
- StatusLine: single-line status updates with auto-clear

All utilities auto-detect terminal width, clean up on Ctrl+C,
and degrade gracefully if stdout isn't a TTY.
"""

import itertools
import os
import shutil
import sys
import threading
import time
from contextlib import contextmanager
from typing import Generator, Optional


# ─── Terminal ─────────────────────────────────────────────────────────

def _terminal_width(default: int = 60) -> int:
    """Get terminal width, falling back to `default` if undetectable."""
    return (shutil.get_terminal_size().columns or default) - 1


def _is_tty() -> bool:
    """Is stderr a real terminal?"""
    return sys.stderr.isatty()


# ─── Braille Spinner ──────────────────────────────────────────────────

_BRAILLE_FRAMES = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]
_BRAILLE_CYCLE = itertools.cycle(_BRAILLE_FRAMES)

_DOT_FRAMES = ["⣀", "⣠", "⣤", "⣦", "⣶", "⣾", "⣽", "⣻", "⣟", "⡿", "⢿", "⢾", "⢶"]
_DOT_CYCLE = itertools.cycle(_DOT_FRAMES)

_SIMPLE_CYCLE = itertools.cycle(["|", "/", "-", "\\"])


class Spinner:
    """Animated indeterminate spinner for CLI operations.

    Usage:
        with Spinner("connecting to relay..."):
            do_slow_thing()
        # → "✅ connecting to relay..." printed on exit

    Or manually:
        spinner = Spinner("working")
        spinner.start()
        do_work()
        spinner.stop("✅ done")
    """

    def __init__(
        self,
        message: str = "",
        *,
        frames: Optional[list[str]] = None,
        stream=None,
        enabled: Optional[bool] = None,
    ):
        self._message = message
        self._frames = frames or _BRAILLE_FRAMES
        self._stream = stream or sys.stderr
        self._enabled = _is_tty() if enabled is None else enabled
        self._thread: Optional[threading.Thread] = None
        self._stop_event = threading.Event()
        self._final_message: Optional[str] = None
        self._start_time: float = 0.0

    def start(self, message: Optional[str] = None):
        """Start the spinner in a background thread."""
        if message is not None:
            self._message = message
        if not self._enabled:
            self._write(f"  {self._message}...")
            return
        self._start_time = time.monotonic()
        self._stop_event.clear()
        self._thread = threading.Thread(target=self._spin, daemon=True)
        self._thread.start()

    def _spin(self):
        """Animation loop running in background thread."""
        cycle = itertools.cycle(self._frames)
        while not self._stop_event.is_set():
            elapsed = time.monotonic() - self._start_time
            frame = next(cycle)
            self._write(f"\r{frame} {self._message}  ({elapsed:.0f}s)")
            time.sleep(0.08)

    def stop(self, message: Optional[str] = None):
        """Stop the spinner and show final status."""
        if not self._enabled and self._start_time:
            elapsed = time.monotonic() - self._start_time
            self._write(f"  {message or self._message}  ({elapsed:.0f}s)\n")
            return
        self._stop_event.set()
        if self._thread:
            self._thread.join(0.5)
        elapsed = time.monotonic() - self._start_time if self._start_time else 0
        elapsed_str = f"  ({elapsed:.0f}s)" if elapsed > 0.5 else ""
        final = message or self._final_message or f"✅ {self._message}"
        # Clear the spinner line and write final
        self._write(f"\r{' ' * (_terminal_width() - 1)}\r{final}{elapsed_str}\n")

    def fail(self, message: Optional[str] = None):
        """Stop the spinner with a failure message."""
        self.stop(message or f"❌ {self._message}")

    def _write(self, text: str):
        """Write to stderr, flushing immediately."""
        self._stream.write(text)
        self._stream.flush()

    def __enter__(self):
        self.start()
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        if exc_type is not None:
            self.fail()
        else:
            self.stop()
        return False

    @property
    def elapsed(self) -> float:
        return time.monotonic() - self._start_time if self._start_time else 0


# ─── Progress Bar ─────────────────────────────────────────────────────

_BLOCK_FULL = "█"
_BLOCK_LIGHT = "░"
_BLOCK_EIGHTHS = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉", "█"]

# Try to use tqdm if available, fall back to our own
try:
    from tqdm import tqdm as _tqdm

    _HAS_TQDM = True
except ImportError:
    _HAS_TQDM = False


class ProgressBar:
    """Determinate progress bar with percentage, ETA, and message.

    Uses tqdm when available, falls back to a simple unicode bar.

    Usage:
        with ProgressBar(total=100, desc="Downloading") as bar:
            for i in range(100):
                do_step()
                bar.update(1)

    Or as callback:
        bar = ProgressBar(total=10, desc="Pushing")
        bar.start()
        for i in range(10):
            do_step(i)
            bar.update(1)
        bar.stop("✅ Done")
    """

    def __init__(
        self,
        total: int = 0,
        *,
        desc: str = "",
        unit: str = "it",
        stream=None,
        enabled: Optional[bool] = None,
    ):
        self.total = total
        self.desc = desc
        self.unit = unit
        self.stream = stream or sys.stderr
        self.enabled = _is_tty() if enabled is None else enabled
        self._n = 0
        self._start_time: float = 0.0
        self._tqdm_bar = None
        self._final_message: Optional[str] = None

    def start(self, total: Optional[int] = None, desc: Optional[str] = None):
        """Initialize the progress bar."""
        if total is not None:
            self.total = total
        if desc is not None:
            self.desc = desc
        self._n = 0
        self._start_time = time.monotonic()

        if _HAS_TQDM and self.enabled and self.total:
            self._tqdm_bar = _tqdm(
                total=self.total,
                desc=self.desc,
                unit=self.unit,
                file=self.stream,
                leave=False,
                ncols=_terminal_width(),
                bar_format="{desc}: {percentage:3.0f}%|{bar}| {n_fmt}/{total_fmt} [{elapsed}<{remaining}, {rate_fmt}]",
            )
        elif self.total and self.enabled:
            self._render()

    def update(self, n: int = 1):
        """Advance by n steps."""
        if self._tqdm_bar is not None:
            self._tqdm_bar.update(n)
        else:
            self._n += n
            if self.enabled:
                self._render()

    def set_description(self, desc: str):
        """Update the description text."""
        self.desc = desc
        if self._tqdm_bar is not None:
            self._tqdm_bar.set_description(desc)

    def _render(self):
        """Render a simple unicode progress bar."""
        if not self.enabled or not self.total:
            return
        width = min(30, _terminal_width() - 40)
        pct = self._n / max(self.total, 1)
        filled = int(width * pct)
        remainder = (width * pct) - filled
        # Use eighth-block for partial fill
        partial = _BLOCK_EIGHTHS[int(remainder * 8)] if remainder > 0 else ""
        bar = _BLOCK_FULL * filled + partial + _BLOCK_LIGHT * (width - filled - (1 if partial else 0))
        elapsed = time.monotonic() - self._start_time
        self.stream.write(
            f"\r{self.desc}: {pct * 100:3.0f}%|{bar}| {self._n}/{self.total} [{elapsed:.0f}s]"
        )
        self.stream.flush()

    def stop(self, message: Optional[str] = None):
        """Finalize the bar and show completion message."""
        if self._tqdm_bar is not None:
            self._tqdm_bar.close()
            self._tqdm_bar = None
        elapsed = time.monotonic() - self._start_time if self._start_time else 0
        elapsed_str = f"  ({elapsed:.0f}s)" if elapsed > 0.5 else ""
        final = message or self._final_message or f"✅ {self.desc}"
        if self.enabled:
            self.stream.write(f"\r{' ' * (_terminal_width() - 1)}\r{final}{elapsed_str}\n")
            self.stream.flush()

    def fail(self, message: Optional[str] = None):
        """Finalize the bar with a failure message."""
        self.stop(message or f"❌ {self.desc}")

    def __enter__(self):
        self.start()
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        if exc_type is not None:
            self.fail()
        else:
            self.stop()
        return False


# ─── Status Helpers ────────────────────────────────────────────────────

def status(msg: str):
    """Write a status line (overwritable by spinner/progress)."""
    if _is_tty():
        sys.stderr.write(f"\r{' ' * (_terminal_width() - 1)}\r{msg}\n")
    else:
        print(msg)
    sys.stderr.flush()


def step(msg: str):
    """Write a short step indicator."""
    if _is_tty():
        w = _terminal_width()
        trunc = msg[: w - 3]
        sys.stderr.write(f"\r  {trunc}...{' ' * (w - len(trunc) - 5)}\r")
    else:
        sys.stderr.write(f"  {msg}... ")
    sys.stderr.flush()


# ─── Quick test ────────────────────────────────────────────────────────

if __name__ == "__main__":
    print("Testing spinner...")
    with Spinner("doing important work"):
        time.sleep(2)

    print("Testing progress bar...")
    with ProgressBar(total=20, desc="Downloading") as bar:
        for i in range(20):
            time.sleep(0.1)
            bar.update(1)

    print("Done!")

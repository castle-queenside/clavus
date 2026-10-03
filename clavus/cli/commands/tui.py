from __future__ import annotations

"""Clavus CLI command: tui — Launch the TUI dashboard."""
import argparse

def cmd_tui(args: argparse.Namespace) -> None:
    """Launch the Textual TUI."""
    try:
        from clavus.tui import run_tui
    except ImportError:
        print("-- TUI requires textual and httpx.")
        print("   Install with: pip install textual httpx")
        sys.exit(1)
    cfg = ClavusConfig.load()
    url = args.connect or cfg.default_server
    debug = getattr(args, 'debug', False)
    run_tui(url=url, debug=debug)



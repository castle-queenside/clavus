from __future__ import annotations

"""Clavus CLI command: watch — Auto-snapshot on file changes."""
import argparse
import sys
from clavus.watch import watch
from clavus.helpers import get_store_and_project

def cmd_watch(args: argparse.Namespace) -> None:
    """Start the file watcher daemon, or manage the system service.

    clavus watch            — start daemon (foreground, tracks active project)
    clavus watch --once     — take one snapshot if changed and exit
    clavus watch install    — install as system service (launchd/systemd)
    clavus watch start      — start the installed service
    clavus watch stop       — stop the service
    clavus watch restart   — stop + start
    clavus watch status    — show if service is running

    The daemon auto-tracks whichever project is active in Clavus. Switch projects
    in the TUI or CLI and the watcher picks it up without restarting.
    """
    sub = getattr(args, 'subcommand', None)

    if sub == 'install':
        from clavus.watch import install_service
        ok = install_service()
        sys.exit(0 if ok else 1)
    elif sub == 'start':
        from clavus.watch import start_service
        ok = start_service()
        sys.exit(0 if ok else 1)
    elif sub == 'stop':
        from clavus.watch import stop_service
        ok = stop_service()
        sys.exit(0 if ok else 1)
    elif sub == 'restart':
        from clavus.watch import stop_service, start_service
        stop_service()
        ok = start_service()
        sys.exit(0 if ok else 1)
    elif sub == 'status':
        from clavus.watch import service_status
        status = service_status()
        if status:
            print(f'[ok] Watch service: {status}')
        else:
            print(f'[WARN]  Service status not available on this platform')
        return
    else:
        # Default: run the daemon. No store/proj args — reads from index.json each poll.
        from clavus.watch import LOG_FILE
        watch(
            cooldown=args.cooldown,
            verbose=not args.quiet,
            once=args.once,
            log_file=LOG_FILE,
        )



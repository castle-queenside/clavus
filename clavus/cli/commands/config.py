from __future__ import annotations

"""Clavus CLI command: config."""
import argparse

def cmd_config(args: argparse.Namespace) -> None:
    """View or edit clavus configuration."""
    cfg = ClavusConfig.load()

    # ── Set a value ──
    if args.key and args.value is not None:
        valid_keys = {"author", "port", "host", "default_server", "default_project"}
        if args.key not in valid_keys:
            print(f"-- Unknown setting '{args.key}'.")
            print(f"   Valid keys: {', '.join(sorted(valid_keys))}")
            return
        setattr(cfg, args.key, args.value)
        cfg.save()
        print(f"[ok] {args.key} = {args.value}")
        return

    # ── Show single value ──
    if args.key:
        val = getattr(cfg, args.key, "")
        if isinstance(val, str):
            val = f"'{val}'"
        print(f"  {args.key} = {val}")
        return

    # ── Show all ──
    print(f"  Clavus Configuration ({CONFIG_PATH})")
    print()
    for k, v in cfg.to_dict().items():
        print(f"    {k} = {v}")
    print()
    print(f"  Run 'clavus setup' for interactive setup.")
    print(f"  Run 'clavus config <key> <value>' to set a value.")



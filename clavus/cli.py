"""
clavus CLI — backward-compatibility entry point.

All CLI logic moved to clavus.cli (clavus/cli/__init__.py).
This file re-exports main() for: python -m clavus.cli
"""
from clavus.cli import main

__all__ = ["main"]

if __name__ == "__main__":
    main()

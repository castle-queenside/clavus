"""Clavus configuration — single source of truth for user settings.

Config file: ~/.config/clavus/config.json
Env overrides: CLAVUS_AUTHOR, CLAVUS_PORT, CLAVUS_HOST, CLAVUS_SERVER
CLI flags: highest priority (override both config and env)
"""

from __future__ import annotations

import getpass
import json
import os
from dataclasses import dataclass, field
from pathlib import Path
from typing import Optional

# ─── Defaults ────────────────────────────────────────────────────────────────

DEFAULT_PORT = 7890
DEFAULT_HOST = "0.0.0.0"
DEFAULT_AUTHOR = getpass.getuser()
DEFAULT_SERVER = f"http://localhost:{DEFAULT_PORT}"
DEFAULT_PROJECTS_DIR = str(Path.home() / "Clavus" / "Projects")
CONFIG_DIR = Path.home() / ".config" / "clavus"
CONFIG_PATH = CONFIG_DIR / "config.json"


@dataclass
class ClavusConfig:
    """User-facing configuration, resolved from file → env → defaults."""

    author: str = DEFAULT_AUTHOR
    port: int = DEFAULT_PORT
    host: str = DEFAULT_HOST
    default_server: str = DEFAULT_SERVER
    default_project: str = ""
    projects_dir: str = DEFAULT_PROJECTS_DIR

    # Derived (computed from host + port)
    server_url: str = DEFAULT_SERVER

    def __post_init__(self):
        if not self.server_url or self.server_url == DEFAULT_SERVER:
            self.server_url = f"http://{self.host}:{self.port}"

    def to_dict(self) -> dict:
        return {
            "author": self.author,
            "port": self.port,
            "host": self.host,
            "default_server": self.default_server,
            "default_project": self.default_project,
        }

    @classmethod
    def load(cls) -> ClavusConfig:
        """Load config: file → env → defaults. Env vars override file values."""
        cfg = cls._from_file()
        cfg = cls._apply_env(cfg)
        return cfg

    @classmethod
    def _from_file(cls) -> ClavusConfig:
        if not CONFIG_PATH.exists():
            return cls()
        try:
            data = json.loads(CONFIG_PATH.read_text())
            return cls(
                author=data.get("author", DEFAULT_AUTHOR),
                port=data.get("port", DEFAULT_PORT),
                host=data.get("host", DEFAULT_HOST),
                default_server=data.get("default_server", DEFAULT_SERVER),
                default_project=data.get("default_project", ""),
                projects_dir=data.get("projects_dir", DEFAULT_PROJECTS_DIR),
            )
        except (json.JSONDecodeError, OSError):
            return cls()

    @staticmethod
    def _apply_env(cfg: ClavusConfig) -> ClavusConfig:
        if author := os.environ.get("CLAVUS_AUTHOR"):
            cfg.author = author
        if port := os.environ.get("CLAVUS_PORT"):
            try:
                cfg.port = int(port)
            except ValueError:
                pass
        if host := os.environ.get("CLAVUS_HOST"):
            cfg.host = host
        if server := os.environ.get("CLAVUS_SERVER"):
            cfg.default_server = server
            cfg.server_url = server
        if projects_dir := os.environ.get("CLAVUS_PROJECTS_DIR"):
            cfg.projects_dir = projects_dir
        # Recompute server URL if host/port changed but CLAVUS_SERVER wasn't set
        if "CLAVUS_SERVER" not in os.environ:
            cfg.server_url = f"http://{cfg.host}:{cfg.port}"
        return cfg

    def save(self):
        """Persist current config to ~/.config/clavus/config.json."""
        CONFIG_DIR.mkdir(parents=True, exist_ok=True)
        CONFIG_PATH.write_text(json.dumps(self.to_dict(), indent=2) + "\n")

    def set(self, key: str, value):
        """Set a config value in-memory (call .save() to persist)."""
        if hasattr(self, key):
            setattr(self, key, value)
        # Also track in to_dict for unknown keys that should persist
        self._extra = getattr(self, '_extra', {})
        self._extra[key] = value

    def to_dict(self) -> dict:
        d = {
            "author": self.author,
            "port": self.port,
            "host": self.host,
            "default_server": self.default_server,
            "default_project": self.default_project,
            "projects_dir": self.projects_dir,
        }
        # Include any extra keys set via .set()
        for k, v in getattr(self, '_extra', {}).items():
            if k not in d:
                d[k] = v
        return d

    @classmethod
    def merge_cli(cls, cfg: ClavusConfig, **overrides) -> ClavusConfig:
        """Apply CLI flag overrides on top of resolved config. Returns new instance."""
        kwargs = cfg.to_dict()
        for key, val in overrides.items():
            if val is not None and val != "":
                if key == "port":
                    try:
                        val = int(val)
                    except (ValueError, TypeError):
                        continue
                kwargs[key] = val
        return cls(**kwargs)

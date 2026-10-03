"""Shared pytest fixtures for Clavus tests."""
import tempfile
from pathlib import Path
from clavus.store import BlobStore


def temp_store():
    """Provide a temporary BlobStore for tests."""
    with tempfile.TemporaryDirectory() as td:
        store = BlobStore(clavus_dir=Path(td))
        store.init()
        yield store

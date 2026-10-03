"""Tests for the clavus watch daemon."""
import pytest


def test_watch_module_import():
    """watch module loads without error."""
    from clavus import watch
    assert callable(watch.watch)
    assert callable(watch.watch_once)

"""Test the Ableton/Live project parser."""
from pathlib import Path
from clavus.parser import parse_als, project_summary


def test_parser_import():
    """Parser module loads without error."""
    assert parse_als is not None
    assert project_summary is not None

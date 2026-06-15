#!/usr/bin/env python3
"""Test resolve_project_als — three-fallback path resolver for cross-machine pulls.

Self-contained, uses temp dirs. Never touches real ~/Music or Desktop.
"""
import os
import shutil
import sys
import tempfile
from pathlib import Path

# Isolate get_projects_dir() by giving it a tempdir to point at.
# resolve_project_als calls get_projects_dir() which uses platform-specific paths.
# We test the function in isolation by setting the env the helpers module reads.
_TMP = Path(tempfile.mkdtemp(prefix="clavus_path_test_"))
os.environ["CLAVUS_DIR"] = str(_TMP / ".clavus")

sys.path.insert(0, str(Path(__file__).parent))

from clavus.helpers import resolve_project_als, get_projects_dir


def _make_als(path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("fake .als content for test")


# ─── Tests ──────────────────────────────────────────────────────────────

def test_root_als_works() -> None:
    """If root_als from index is valid, return it as-is."""
    print("\n═══ 1. root_als exists → return root_als ═══")
    als = _TMP / "explicit_path.als"
    _make_als(als)
    result = resolve_project_als("myproj", str(als))
    assert result == als, f"expected {als}, got {result}"
    print(f"  ✅ Returned root_als: {result}")


def test_fallback_to_projects_dir_flat() -> None:
    """If root_als is empty/wrong, try projects_dir/proj_name/proj_name.als."""
    print("\n═══ 2. Fallback: projects_dir/proj_name/proj_name.als ═══")
    # Monkey-patch get_projects_dir to return our temp dir
    proj_dir = _TMP / "Music" / "myproj"
    expected = proj_dir / "myproj.als"
    _make_als(expected)

    import clavus.helpers as h
    original = h.get_projects_dir
    h.get_projects_dir = lambda: _TMP / "Music"
    try:
        result = resolve_project_als("myproj", "")
    finally:
        h.get_projects_dir = original

    assert result == expected, f"expected {expected}, got {result}"
    print(f"  ✅ Found via flat fallback: {result}")


def test_fallback_to_projects_dir_with_project_subdir() -> None:
    """Third fallback: projects_dir/proj_name/'proj_name Project'/proj_name.als."""
    print("\n═══ 3. Fallback: projects_dir/proj_name/proj_name Project/proj_name.als ═══")
    proj_dir = _TMP / "Music2" / "otherproj" / "otherproj Project"
    expected = proj_dir / "otherproj.als"
    _make_als(expected)

    # Also create a non-matching file to make sure ordering is right
    wrong = _TMP / "Music2" / "otherproj" / "otherproj.als"
    # Don't create it — the "Project" subdir version should win

    import clavus.helpers as h
    original = h.get_projects_dir
    h.get_projects_dir = lambda: _TMP / "Music2"
    try:
        result = resolve_project_als("otherproj", "")
    finally:
        h.get_projects_dir = original

    assert result == expected, f"expected {expected}, got {result}"
    print(f"  ✅ Found via 'Project' subdir fallback: {result}")


def test_none_when_nothing_exists() -> None:
    """If none of the candidates exist, return None (not a crash)."""
    print("\n═══ 4. Nothing exists → return None ═══")
    import clavus.helpers as h
    original = h.get_projects_dir
    h.get_projects_dir = lambda: _TMP / "EmptyMusic"
    try:
        result = resolve_project_als("ghost", "")
    finally:
        h.get_projects_dir = original

    assert result is None, f"expected None, got {result}"
    print("  ✅ Returned None gracefully")


def test_priority_root_als_beats_fallback() -> None:
    """If root_als exists AND a fallback exists, root_als wins."""
    print("\n═══ 5. root_als takes priority over fallbacks ═══")
    primary = _TMP / "primary.als"
    fallback = _TMP / "Music3" / "conflict" / "conflict.als"
    _make_als(primary)
    _make_als(fallback)

    import clavus.helpers as h
    original = h.get_projects_dir
    h.get_projects_dir = lambda: _TMP / "Music3"
    try:
        result = resolve_project_als("conflict", str(primary))
    finally:
        h.get_projects_dir = original

    assert result == primary, f"expected primary {primary}, got {result}"
    print(f"  ✅ root_als won priority race: {result}")


# ─── Run all ────────────────────────────────────────────────────────────

if __name__ == "__main__":
    try:
        test_root_als_works()
        test_fallback_to_projects_dir_flat()
        test_fallback_to_projects_dir_with_project_subdir()
        test_none_when_nothing_exists()
        test_priority_root_als_beats_fallback()
        print(f"\n✅ All resolve_project_als tests passed! (cleaned up {_TMP})")
    except AssertionError as e:
        print(f"\n❌ FAIL: {e}")
        sys.exit(1)
    finally:
        shutil.rmtree(_TMP, ignore_errors=True)

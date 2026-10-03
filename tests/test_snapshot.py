"""Test the snapshot engine with a real or fixture project."""
import pytest, tempfile, shutil
from pathlib import Path
from clavus.store import BlobStore


@pytest.fixture
def temp_store():
    tmp = Path(tempfile.mkdtemp(prefix="clavus_snap_test_"))
    store = BlobStore(clavus_dir=tmp / ".clavus")
    store.init()
    yield store
    shutil.rmtree(tmp, ignore_errors=True)


FIXTURE = Path(__file__).parent.parent / "fixtures" / "test_project.als"


@pytest.mark.skipif(not FIXTURE.exists(), reason="Fixture not found — generate with python3 fixtures/gen_fixture.py")
def test_snapshot_save_and_load(temp_store):
    """Parse fixture, save snapshot, load it back."""
    from clavus.parser import parse_als
    project = parse_als(str(FIXTURE))
    snap = temp_store.save_snapshot(project, message="test snapshot")
    loaded = temp_store.load_snapshot(snap.hash)
    assert loaded.hash == snap.hash
    assert loaded.message == "test snapshot"


def test_blobstore_init_creates_dir(temp_store):
    """init() creates the expected directory structure."""
    assert (temp_store.root / "objects").exists()
    assert (temp_store.root / "refs").exists()
    assert (temp_store.root / "index.json").exists()


def test_schema_version_in_new_index(temp_store):
    """New index should have _schema_version set."""
    import json
    index = json.loads(temp_store.index_path.read_text())
    assert index.get("_schema_version") == 2

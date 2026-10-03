"""Test CLI commands in isolation."""
import pytest, tempfile, shutil, os
from pathlib import Path
from argparse import Namespace
from clavus.store import BlobStore, ClavusProject


@pytest.fixture
def temp_clavus():
    tmp = Path(tempfile.mkdtemp(prefix="clavus_cli_test_"))
    clavus_dir = tmp / ".clavus"
    os.environ["CLAVUS_DIR"] = str(clavus_dir)
    store = BlobStore(clavus_dir=clavus_dir)
    store.init()
    yield store, clavus_dir
    shutil.rmtree(tmp, ignore_errors=True)


def test_blobstore_basic_init(temp_clavus):
    """BlobStore.init() creates directory structure."""
    store, clavus_dir = temp_clavus
    assert clavus_dir.exists()
    assert (clavus_dir / "index.json").exists()


def test_set_and_get_project(temp_clavus):
    """set_index / get_index round-trips a project."""
    store, _ = temp_clavus
    proj = ClavusProject(
        name="TestProject",
        root_als="/tmp/TestProject.als",
        created_at=0.0,
        head=None,
    )
    store.set_index(proj)
    loaded = store.get_index("TestProject")
    assert loaded is not None
    assert loaded.name == "TestProject"
    assert loaded.root_als == "/tmp/TestProject.als"


def test_list_projects(temp_clavus):
    """list_projects returns all tracked projects."""
    store, _ = temp_clavus
    for name in ["Alpha", "Beta"]:
        proj = ClavusProject(name=name, root_als=f"/tmp/{name}.als", created_at=0.0)
        store.set_index(proj)
    projects = store.list_projects()
    names = {p.name for p in projects}
    assert "Alpha" in names
    assert "Beta" in names

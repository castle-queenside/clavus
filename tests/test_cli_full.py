"""Full CLI workflow tests (requires fixtures/gen_fixture.py to generate test_project.als)."""
import pytest, tempfile, shutil, os
from pathlib import Path
from argparse import Namespace
from clavus.store import BlobStore, ClavusProject


FIXTURE = Path(__file__).parent / "fixtures" / "test_project.als"

pytestmark = pytest.mark.skipif(
    not FIXTURE.exists(),
    reason="Fixture not found — run: python3 fixtures/gen_fixture.py"
)


@pytest.fixture
def fixture_als():
    """Path to the test_project.als fixture."""
    return FIXTURE


@pytest.fixture
def temp_clavus():
    tmp = Path(tempfile.mkdtemp(prefix="clavus_full_test_"))
    clavus_dir = tmp / ".clavus"
    os.environ["CLAVUS_DIR"] = str(clavus_dir)
    store = BlobStore(clavus_dir=clavus_dir)
    store.init()
    yield store, tmp
    shutil.rmtree(tmp, ignore_errors=True)


def test_init_creates_project(temp_clavus, fixture_als):
    """clavus init creates a project entry in the index."""
    from clavus.cli import cmd_init
    store, tmp = temp_clavus
    test_als = tmp / "Test Project.als"
    shutil.copy2(str(fixture_als), str(test_als))
    args = Namespace(path=str(test_als), command="init", clavus_dir=str(store.root))
    cmd_init(args)
    assert store.get_index("Test Project") is not None


def test_schema_version_migration(temp_clavus):
    """New projects get _schema_version: 2 set on first set_index."""
    import json
    store, _ = temp_clavus
    proj = ClavusProject(name="SchemaTest", root_als="/tmp/x.als", created_at=0.0)
    store.set_index(proj)
    index = json.loads(store.index_path.read_text())
    assert index["_schema_version"] == 3
    assert index["SchemaTest"]["active_remote"] == ""
    assert index["SchemaTest"]["shared"] is True

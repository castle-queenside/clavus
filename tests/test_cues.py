#!/usr/bin/env python3
"""Test the threaded cue system end-to-end. Self-contained, uses temp dirs."""
import sys, os, shutil, tempfile, time
from pathlib import Path
from argparse import Namespace

import pytest

TMP = Path(tempfile.mkdtemp(prefix="clavus_test_"))
CLAVUS_DIR = TMP / ".clavus"
os.environ["CLAVUS_DIR"] = str(CLAVUS_DIR)

FIXTURE = Path(__file__).parent.parent / "fixtures" / "test_project.als"

test_dir = TMP / "project"
test_dir.mkdir(parents=True, exist_ok=True)

from clavus.cues import CueStore
from clavus.store import BlobStore


@pytest.fixture(autouse=True)
def fresh_store():
    """Reset CLAVUS_DIR for each test."""
    old = os.environ.get("CLAVUS_DIR")
    os.environ["CLAVUS_DIR"] = str(CLAVUS_DIR)
    store = BlobStore(clavus_dir=CLAVUS_DIR)
    store.init()
    yield store
    if old is not None:
        os.environ["CLAVUS_DIR"] = old


def test_cue_add_and_list():
    cue_store = CueStore("test_project")
    cue = cue_store.add_cue("Test cue", "1.1.1", author="tester")
    assert cue.text == "Test cue"
    assert cue.position == "1.1.1"
    assert cue.status == "pending"

    cues = cue_store.list_cues()
    assert any(c.text == "Test cue" for c in cues)


def test_cue_reply():
    cue_store = CueStore("test_project")
    cue = cue_store.add_cue("Reply test", "2.0.0")
    reply = cue_store.reply(cue.id, "This is a reply", author="tester")
    assert reply is not None
    assert reply.text == "This is a reply"
    # Reload to see the reply
    cue = cue_store.get_cue(cue.id)
    assert cue.thread_length == 1


def test_cue_resolve():
    cue_store = CueStore("test_project")
    cue = cue_store.add_cue("To resolve", "3.0.0")
    resolved = cue_store.resolve(cue.id, note="done")
    assert resolved.status == "resolved"


def test_cue_assign_and_unassign():
    cue_store = CueStore("test_project")
    cue = cue_store.add_cue("Assignment test", "4.0.0")
    cue_store.assign(cue.id, "alice")
    cue = cue_store.get_cue(cue.id)
    assert cue.assignee == "alice"

    cue_store.unassign(cue.id)
    cue = cue_store.get_cue(cue.id)
    assert cue.assignee == ""

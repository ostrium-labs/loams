"""The fixtures' own tests (UP1 Task 19)."""

from __future__ import annotations

import os

import pytest

import fixtures
import run
from fixtures import collections, graph, house, live
from lib import Stack


def stack_on(engine, closed_port):
    return Stack(engine=engine.url, qdrant=closed_port, es=closed_port, durable=closed_port,
                 tidb="127.0.0.1:1", pg=None, key=None)


# Everything but the stream and link API, which the fake does not have.
PRODUCTS = [m for m in fixtures.ALL if m.NAME != "streams_links"]


def test_write_then_check_on_same_release_passes(engine, closed_port):
    stack = stack_on(engine, closed_port)
    state = run.write(stack, PRODUCTS)
    assert state["products"]["collections"]["status"] == "written"
    assert run.check(stack, state, products=PRODUCTS) == []
    state = run.write_new(stack, state, products=PRODUCTS)
    assert run.check(stack, state, "present", products=PRODUCTS) == []
    # A rollback from the snapshot takes the new write away.
    del engine.docs[100]
    assert run.check(stack, state, "absent", products=PRODUCTS) == []
    assert run.check(stack, state, "present", products=PRODUCTS) == ["collections (new): document 100 is missing"]


@pytest.mark.skipif(not os.environ.get("LOAMS_UPGRADE_ENGINE"), reason="needs LOAMS_UPGRADE_ENGINE (a running stack)")
def test_write_then_check_on_a_real_stack():
    stack = Stack(engine=os.environ["LOAMS_UPGRADE_ENGINE"], key=os.environ.get("LOAMS_API_KEY"),
                  tidb=os.environ.get("LOAMS_UPGRADE_TIDB", "tidb:4000"))
    state = run.write(stack)
    assert run.check(stack, state) == []


def test_check_detects_a_changed_document(engine, closed_port):
    stack = stack_on(engine, closed_port)
    state = run.write(stack, [collections])
    engine.docs[1] = {"body": "changed", "tenant": "t1"}
    problems = run.check(stack, state, products=[collections])
    assert any(p.startswith("collections: document 1:") for p in problems), problems
    assert any("search 'lovelace'" in p for p in problems), problems


def test_skips_products_the_previous_release_lacks(engine, closed_port):
    stack = stack_on(engine, closed_port)
    state = run.write(stack, PRODUCTS)
    products = state["products"]
    for name in ("graph", "live", "house"):
        assert products[name]["status"] == "skipped", products[name]
        assert "does not serve" in products[name]["reason"]
    for name in ("qdrant", "es", "durable", "postgres", "keys", "sql"):
        assert products[name]["status"] == "skipped", products[name]
    # Skipped products are not checked.
    assert run.check(stack, state, products=PRODUCTS) == []
    # Served, the same release writes them.
    engine.api_versions.append("loams.graph.v1")
    assert graph.NAME in [m.NAME for m in fixtures.ALL]


def test_house_fixture_uses_ddl_until_insert_ships():
    ddl = house.statements(insert=False)
    assert ddl and all(not s.upper().startswith("INSERT") for s in ddl)
    assert any(s.startswith("CREATE TABLE") for s in ddl)
    assert any(s.upper().startswith("INSERT") for s in house.statements(insert=True))
    # The release says when its House takes INSERT.
    assert Stack(instance={"features": {"house.insert": True}}).feature("house.insert")
    assert not Stack(instance={"features": {}}).feature("house.insert")
    assert live.NAME == "live"

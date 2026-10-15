"""One module per product (design §54 §14 step 2). Each has:

- `write(stack) -> dict`: writes the fixture with the APIs the previous release
  serves and returns what a read must give back; raises `Skip` when the
  release (or this stack) does not have the product;
- `check(stack, rec) -> list[str]`: reads it back through every API it was
  written with; the problems, empty when it reads as written;
- `write_new(stack) -> dict` and `check_new(stack, rec, present) -> list[str]`:
  one new item after the upgrade, and whether it is there (after a rollback
  from the snapshot it must not be; with `--keep-writes` it must).
"""

from . import (
    collections,
    durable,
    es,
    graph,
    house,
    keys,
    live,
    postgres,
    qdrant,
    sql,
    streams_links,
)

ALL = [collections, streams_links, qdrant, es, live, graph, sql, postgres, house, durable, keys]

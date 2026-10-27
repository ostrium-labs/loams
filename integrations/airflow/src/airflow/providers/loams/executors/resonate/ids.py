# SPDX-License-Identifier: Apache-2.0
"""Deterministic Resonate identifiers for Airflow work (D2091, D2092).

A task instance try maps to exactly one durable promise:

    af.<ns>.<dag_id>.<run_id>:<task_id>.<map_index>.<try_number>

- The part before the first ``:`` is the promise's *origin* (Resonate's
  partition key), so every try of every task in one Dag run shares an origin.
  ``:`` is reserved by Resonate as the origin separator and must never appear
  inside the origin.
- Each component is escaped so that it cannot contain the separators: every
  byte outside ``[A-Za-z0-9_-]`` becomes ``%XX`` (``.`` is ``%2E`` and ``:`` is
  ``%3A``). Run ids such as ``manual__2026-10-27T10:00:00+00:00`` therefore
  round-trip, and the encoding is injective.
- Some stores keep ids in 255-byte columns, so an id longer than
  :data:`MAX_ID_BYTES` uses the hashed form, still deterministic::

      af.<ns>.h<sha256(dag, run)[:32]>:h<sha256(task)[:32]>.<map_index>.<try_number>

The same inputs always give the same id, on any scheduler, after any restart,
which is what lets a restarted scheduler adopt work by id alone.
"""

from __future__ import annotations

import hashlib
import re
from dataclasses import dataclass

#: Ids above this length use the hashed form (MySQL and SQL stores keep ids in
#: ``VARCHAR(255)``; TiKV allows 8 KiB keys).
MAX_ID_BYTES = 255

#: The id prefix for Airflow work in Resonate.
PREFIX = "af"

#: Suffix of the marker promise a worker creates before it starts a task
#: process (D2093). Its existence means "a process for this try has started".
STARTED_SUFFIX = "started"

_SAFE = frozenset(b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-")
_LABEL_OK = re.compile(r"^[a-z0-9]([a-z0-9-]*[a-z0-9])?$")


def escape(component: str) -> str:
    """Percent-escape every byte of ``component`` outside ``[A-Za-z0-9_-]``."""
    out: list[str] = []
    for byte in component.encode("utf-8"):
        if byte in _SAFE:
            out.append(chr(byte))
        else:
            out.append(f"%{byte:02X}")
    return "".join(out)


def unescape(component: str) -> str:
    """Invert :func:`escape`."""
    raw = bytearray()
    i = 0
    while i < len(component):
        ch = component[i]
        if ch == "%":
            raw.append(int(component[i + 1 : i + 3], 16))
            i += 3
        else:
            raw.extend(ch.encode("ascii"))
            i += 1
    return raw.decode("utf-8")


def _digest(*parts: str) -> str:
    h = hashlib.sha256()
    for part in parts:
        h.update(part.encode("utf-8"))
        h.update(b"\x00")
    return h.hexdigest()[:32]


@dataclass(frozen=True)
class TaskRef:
    """The fields of an Airflow ``TaskInstanceKey`` an id is derived from."""

    dag_id: str
    run_id: str
    task_id: str
    try_number: int
    map_index: int = -1


def task_promise_id(ns: str, ref: TaskRef) -> str:
    """The promise id for one try of one task instance (D2091)."""
    ns_e = escape(ns)
    map_e = escape(str(ref.map_index))
    try_e = escape(str(ref.try_number))
    plain = f"{PREFIX}.{ns_e}.{escape(ref.dag_id)}.{escape(ref.run_id)}:{escape(ref.task_id)}.{map_e}.{try_e}"
    if len(plain.encode("utf-8")) <= MAX_ID_BYTES:
        return plain
    return f"{PREFIX}.{ns_e}.h{_digest(ref.dag_id, ref.run_id)}:h{_digest(ref.task_id)}.{map_e}.{try_e}"


def callback_promise_id(ns: str, callback_id: str) -> str:
    """The promise id for an Airflow ``ExecuteCallback`` workload."""
    plain = f"{PREFIX}.{escape(ns)}.cb:{escape(callback_id)}"
    if len(plain.encode("utf-8")) <= MAX_ID_BYTES:
        return plain
    return f"{PREFIX}.{escape(ns)}.cb:h{_digest(callback_id)}"


def started_marker_id(promise_id: str) -> str:
    """The id of the "process started" marker for ``promise_id`` (D2093).

    It sits one lineage segment below the task's promise, so it shares the
    task's origin.
    """
    sep = "." if ":" in promise_id else ":"
    return f"{promise_id}{sep}{STARTED_SUFFIX}"


def origin_of(promise_id: str) -> str:
    """Everything before the first ``:``, as Resonate computes it."""
    return promise_id.split(":", 1)[0]


def _label(value: str) -> str:
    """One DNS-style label: lowercase ``[a-z0-9-]``, plus a short hash if lossy."""
    lowered = value.lower()
    cleaned = re.sub(r"[^a-z0-9-]+", "-", lowered).strip("-") or "x"
    if cleaned != value or not _LABEL_OK.match(cleaned):
        cleaned = f"{cleaned[:40].rstrip('-')}-{_digest(value)[:8]}"
    return cleaned[:63]


def target_group(ns: str, queue: str, team: str | None = None) -> str:
    """The Resonate target group for an Airflow queue (D2092).

    ``af.<ns>.<queue>``, or ``af.<ns>.<team>.<queue>`` for a team's executor.
    Every label is a lowercase ``[a-z0-9-]`` hostname label; a name that had to
    be changed to fit gets an 8-hex suffix of its hash so two different queues
    never collapse into one group. Labels never contain ``.``, so the label
    count tells the two shapes apart.
    """
    labels = [PREFIX, _label(ns)]
    if team:
        labels.append(_label(team))
    labels.append(_label(queue))
    return ".".join(labels)


def anycast_address(group: str) -> str:
    """The ``resonate:target`` address any worker polling ``group`` may claim."""
    return f"poll://any@{group}"


def validate_group(group: str) -> str:
    """Return ``group`` if every label is a valid hostname label, else raise."""
    labels = group.split(".")
    if not labels or not all(_LABEL_OK.match(label) for label in labels):
        msg = f"invalid Resonate target group {group!r}: labels must be [a-z0-9-]"
        raise ValueError(msg)
    return group

![Loams — Your data. Your bucket.](../../docs/assets/loams-banner.svg)

# Loams SDK for Python

One `Loams` object over one Loams instance, speaking the unified Connect API.
The module surface is generated from `proto/` by `scripts/sdk/gen.sh python`;
the runtime below it is hand-written and obeys the contract in
[`docs/sdk/runtime-contract.md`](../../docs/sdk/runtime-contract.md).

## Install

```
pip install loams
```

Not on PyPI yet. The distribution name is reserved but publishing is blocked
until registry ownership is confirmed (D400), so for now install from a
checkout:

```
uv sync --all-extras --dev     # from sdks/python
```

The runtime dependency is [`connect-python`](https://pypi.org/project/connect-python/),
which provides the `connectrpc` import name. That is not a typo — the
distribution and the module it installs have different names.

## Use

```python
from loams import Loams

loams = Loams("https://acme.loams.dev", api_key="...")
print(loams.instance.version())
```

Four modules hang off the client: `loams.instance` (what this instance is),
`loams.live` (the live-sync session half, whose package is `unstable`),
`loams.tables` (the table half of the same service), and `loams.system` (the
module catalogue, feature detection and the version check).

### Auth

Pass `api_key` for a script or CI job that holds one — it does not expire, so
there is nothing to refresh. Pass `auth` for everything else: `env_token()`,
`oidc_exchange()`, or your own `TokenSource`. The two are mutually exclusive
because they answer the same question, and passing both raises.

### Protocol and encoding

`protocol` is `"connect"` (default), `"grpc"` or `"grpc-web"` — the one port
speaks all three; this picks which one this client uses. `proto_json=True`
sends the proto3 JSON mapping that `curl` sends; the default is protobuf,
which is smaller.

### Consistency

`session_consistency=False` by default: every read is then `STRONG` on its own,
which is correct but does not give read-your-writes across processes. Turn it
on (D609) to hold a session consistency token across calls.

## What is not here yet

These are refused by name rather than stubbed, so a caller finds out at the
call instead of at runtime:

- **The hybrid query builder.** Blocked on API1 Task 2. There is no
  hand-written builder here; when the proto lands, the generator emits one.
- **Bulk ingest and bulk results.** Blocked on API1 Tasks 3 and 4. They go over
  Arrow Flight SQL through ADBC, which is why the `flight`, `arrow` and
  `polars` extras already exist — the code that uses them lands with the write
  RPCs.
- **`listAll` per call.** Blocked on the same API1 tasks; paging is explicit.

## Development

```
uv sync --all-extras --dev
uv run pytest -q
uv run mypy --strict src
./scripts/sdk/gen.sh python    # regenerate the facade from proto/
```

Generated code is never hand-edited: CI regenerates and fails on a diff, so a
proto change shows up as a dependency bump rather than a silent drift.

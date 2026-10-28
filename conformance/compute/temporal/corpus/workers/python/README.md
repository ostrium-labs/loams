# The corpus's Python worker (temporalio 1.34.0)

Lane TSDK-python (D2135–D2139): the official Python SDK
([`temporalio`](https://pypi.org/project/temporalio/) 1.34.0, on sdk-core),
run with [uv](https://docs.astral.sh/uv/) 0.12.24, at parity with the
TypeScript worker: every scenario of `../../scenarios.json` except the
Go-only ones. Status: preview, **not yet run**; the first run is the
`sdk-leg (python)` job of `.github/workflows/temporal-gateway.yml`.

| Path | What |
|---|---|
| `corpus_py/scenarios.py` | the scenario list (manifest.py checks it against scenarios.json) |
| `corpus_py/workflows.py`, `activities.py` | one workflow class per scenario, activities named as Go's |
| `corpus_py/drive.py` | the starter: one driver per scenario |
| `corpus_py/replay.py` | the SDK's `Replayer` on a directory of histories |
| `tests/` | stdlib-only table checks |

```sh
uv sync --frozen
uv run python -m corpus_py generate 127.0.0.1:7233 default /tmp/generate-python.json   # worker + starter
uv run python -m corpus_py worker 127.0.0.1:7233 default                               # worker only
uv run python -m corpus_py start 127.0.0.1:7233 default hello                          # one scenario's starter
uv run python -m corpus_py replay ../../histories/python                               # replayer
uv run python -m unittest discover -s tests
```

Python, like TypeScript, has no `SideEffect`, so
`side_effect_and_version_marker` records the version marker only. The
histories are exported by the Go exporter with `-prefix corpus-python-`, as
TypeScript's are. `histories/python/` stays empty until `generate.sh
--langs python` runs against compat mode (D2137).

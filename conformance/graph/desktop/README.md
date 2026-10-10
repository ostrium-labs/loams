# The desktop Graph page's contract

GR1 Task 7 (design §48 §18.2). Each `*.json` file here pins calls the desktop Graph page makes and
the answers it reads, in proto3 JSON (Connect JSON). Three readers share them:

- `crates/loams-graph/tests/desktop_contract.rs`: the server, seeded with `movies.gql`, answers
  every `loams.graph.v1` call as the fixture says (`server_answers_match_desktop_fixtures`), and
  so does the seeded mock over Connect JSON (`mock_answers_match_desktop_fixtures`).
  `crates/loams/tests/connect_graph.rs` checks `instance.json` against `GetInstance`.
- `crates/loams-apps-mock` answers from them (`src/graph.rs`), so the page behaves the same against
  the mock and against `loams dev`.
- The page's own tests (`web/plugins/graph`, GR1 Task 8) replay them through a fake transport.

| File | Calls |
|---|---|
| `instance.json` | `GetInstance`: the `loams.graph.v1` row the page detects Graph from |
| `list_graphs.json` | `ListGraphs`, page size 50: `kg` (LINKED) and `movies` (OWNED) in `default` |
| `schema.json` | `GetSchema` of `movies` |
| `execute_table.json` | `Execute` as the page sends it, with typed cells |
| `execute_graph.json` | `Execute` answering `Node`, `Relationship` and `Path` values |
| `execute_truncated.json` | `Execute` cut at `max_rows` (`truncated`), then `ExecuteStream` of all rows |
| `explain.json` | `Explain` and a PROFILE |
| `error_syntax.json` | A syntax error with `gqlstatus`, `line`, `column` and `length` |
| `error_denied.json` | A write with `read_only` on: `permission_denied`, `graph_read_only` |
| `values.json` | Every `Value` kind (GR1 Task 2's golden, `crates/loams-graph/tests/proto.rs`) |

## Format

A file is `{ "description", "exchanges": [ … ] }`. An exchange is one call:

- `method`: `<package>.<Service>/<Method>`.
- `request`: the request message.
- One answer: `response` (a unary message), `chunks` (a server stream's messages, in order) or
  `error` (`{ code, message, reason, metadata }`: the Connect code in its wire spelling, and the
  `loams.errors.v1.ErrorInfo` reason and metadata).
- `ignore`: paths not compared because they differ per server or per run. A path is keys joined
  by `.`; `key[]` means every element of the array at `key` (`[]` alone: of the answer itself);
  `**.key` means `key` at any depth.
- `contains`: the answer must hold the fixture's fields and array elements rather than equal it.

proto3 JSON leaves out a field at its default: an `id` of 0, an empty list, a `false`. A reader
treats a missing field as its default.

## Regenerating

The answers are the server's. After a change to the server or to `movies.gql`:

```bash
UPDATE_GOLDEN=1 cargo test -p loams-graph --test desktop_contract server_answers_match_desktop_fixtures
```

rewrites every answer, keeping each fixture's old values at its `ignore` paths (ids, times), so an
unrelated regeneration does not churn them. Requests and descriptions are written by hand.

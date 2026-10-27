# The builder corpus

One file per case (SDKP P0d). Every SDK's `<lang>_builders_match_corpus` test
reads every file here, drives its own `Schema`, `Filter` or `Search` builder
from `spec`, and checks the result against `expect`. No server is involved:
the corpus pins client-side encoding, so thirteen builders cannot drift apart.

| Key | Meaning |
|---|---|
| `name` | The case, and the file name |
| `builder` | `schema`, `filter` or `search` |
| `message` | What `expect` is: `google.protobuf.Struct` (the schema) or the proto message, whose proto3 JSON it is |
| `spec` | The builder calls, in the neutral notation below |
| `expect` | The builder's output, as proto3 JSON (field names `lowerCamelCase`, enums by name, 64-bit integers as strings) |
| `scenario_step` | For a `search_*` case, the `app_*` scenario step whose request body this is |

## Notation

A call is `{"op": "<verb>", "args": [...]}`. Verbs are the `snake_case` names
of SDKP §2.4 and §2.6, and each language maps them to its own casing
(`refine_factor` is `refineFactor` in TypeScript and `refine_factor` in
Python). An argument is plain JSON, except that an object with an `op` key is
itself a call: a filter inside `and`, a fusion inside `fuse`, a sort key inside
`sort`. A trailing plain object is the call's options.

- **`schema`**: `spec` is the list of calls on a new `Schema`, then `to_json`.
- **`filter`**: `spec` is one filter call; `and`, `or`, `not` take filter calls.
- **`search`**: `spec` is `{"on": "<collection>", "calls": [...]}`, applied in
  order to `Search.on(collection)`, then `build`.

A numeric `ids` argument is a `uint` document id; a string is a `string` id;
`{"uuid": "…"}` is a `uuid` id.

Compare by decoding both sides into the message (or, for a `Struct`, as JSON
values), not by comparing text: key order and number spelling are not part of
the contract.

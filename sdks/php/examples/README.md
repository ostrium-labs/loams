# Loams PHP examples

Loams is an open-source database for agents: documents, vectors, graph and SQL behind one API.
Docs: <https://loams.dev/docs> · release notes: docs/release/preview.md in the main repo.

These examples target **Loams SDK 0.1.4**. The matching `v0.1.4`
release tag is published. PHP 8.2+ is
required; the examples load the SDK source directly, so Composer is optional.

Start a local engine (`cargo run -p loams -- dev`, or the compose stack), then
check out the exact SDK release and run from its package root:

```sh
git clone --branch v0.1.4 --depth 1 https://github.com/ostrium-labs/loams.git
cd loams/sdks/php
export LOAMS_ENDPOINT=http://127.0.0.1:8080
export LOAMS_NAMESPACE=examples-php
php examples/quickstart/main.php
php examples/approvals/main.php
php examples/status/main.php
php examples/notes/notes.php
```

`LOAMS_ENDPOINT` overrides the engine address (default `http://127.0.0.1:8080`).
The shared helper also accepts legacy `LOAMS_URL` when `LOAMS_ENDPOINT` is unset.
The notes example uses `LOAMS_NAMESPACE`; the commands above scope its data to
`examples-php`. Set `LOAMS_API_KEY` if your engine requires authentication.

Scope: the PHP SDK is preview. `notes/` exercises namespaces, collections, documents,
text/hybrid and direct vector search, experimental graph, SQL and streams. The other
examples use instance discovery and the approvals list API.

| Module family | Example coverage in SDK 0.1.4 |
|---|---|
| `instance` | Covered by quickstart and status. |
| Approvals | Read-only listing covered by `approvals/`; decisions and watches require credentials and are not exercised. |
| `live`, devices | Service APIs are available but not exercised by these examples. |
| `namespaces`, `collections`, `documents`, `search`, `vector`, `graph`, `sql`, `streams` | Covered in `notes/`; graph is optional on experimental server builds. |
| Authentication | Optional `LOAMS_API_KEY` / `LOAMS_TOKEN`; no sign-in provisioning flow is implied. |
| Live admin, tables, system catalogue | No typed client module in the PHP SDK 0.1.4. |
| House | Unavailable as a typed module in SDK 0.1.4. |

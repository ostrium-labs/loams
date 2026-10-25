# Loams PHP examples

Loams is an open-source database for agents: documents, vectors, graph and SQL behind one API.
Docs: <https://loams.dev/docs> · release notes: docs/release/preview.md in the main repo.

These examples target the upcoming **Loams SDK 0.1.2**. The matching `v0.1.2`
release tag must be published before using the checkout below. PHP 8.2+ is
required; the examples load the SDK source directly, so Composer is optional.

Start a local engine (`cargo run -p loams -- dev`, or the compose stack), then
check out the exact SDK release and run from its package root:

```sh
git clone --branch v0.1.2 --depth 1 https://github.com/ostrium-labs/loams.git
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

Scope: the PHP SDK is preview. `notes/` is the app-building API (namespaces,
collections, documents, hybrid search and graph); the others use the instance, live,
approval and device services.

# Loams PHP examples

Loams is an open-source database for agents: documents, vectors, graph and SQL behind one API.
Docs: <https://loams.dev/docs> · release notes: docs/release/preview.md in the main repo.

Run a local engine (`cargo run -p loams -- dev`, or the compose stack), then from the
package root (PHP 8.2+, no Composer install needed):

```sh
php examples/quickstart/main.php
php examples/approvals/main.php
php examples/status/main.php
LOAMS_ENDPOINT=http://127.0.0.1:18080 LOAMS_NAMESPACE=app php examples/notes/notes.php
```

`LOAMS_URL` overrides the engine address (default `http://127.0.0.1:18080`).

Scope: the PHP SDK is preview. `notes/` is the app-building API (namespaces,
collections, documents, hybrid search and graph); the others use the instance, live,
approval and device services.

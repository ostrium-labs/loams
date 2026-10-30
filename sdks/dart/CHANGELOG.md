# Changelog

## 0.1.4

- Released with Loams 0.1.4. Six new reason exceptions for the deployment CA's
  internal service (`JoinTokenUnknownException`, `JoinTokenSpentException`,
  `JoinTokenExpiredException`, `IdentityNotGrantedException`,
  `IdentityRevokedException`, `CaNotInitializedException`); no other API changes.

## 0.1.3

- Released with Loams 0.1.3; no API changes.

## 0.1.2

- Released with Loams 0.1.2; no API changes.

## 0.1.1

- `sql` (`query` with `$n` parameters, `queryArrow`, `SqlModule.records`) and
  `streams` (`create`, `get`, `list`, `produce` with producer deduplication,
  `fetch`, `produceEvents`, `fetchEvents`, `subscribe` as a `Stream`).
- The app-building API: `namespaces`, `collections`, `documents` (`upsert`,
  `writeBatch`, `scrollAll`, …), `search` (text, vector, hybrid with
  RRF/DBSF/weighted, filters, highlights, `select`, `queryAll`), `vector` and
  `graph`; the `Schema`, `Filter` and `Search` builders; `client.collection(name)`;
  `ConsistencyToken` and the session store; the default namespace,
  `LoamsClient.fromEnv`, API keys and per-call timeouts.

## 0.1.0

- First release on pub.dev, as a preview: the API can change before 1.0.

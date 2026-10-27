# Architecture overview

Object storage is the source of truth, in open formats. A stateless gateway and router speak the native, Qdrant, Elasticsearch and Flight SQL protocols; the log is the spine; TiKV (or an embedded Raft metastore) holds metadata and transactions.

- Diagram and summary: [architecture](https://github.com/ostrium-labs/loams/blob/dev/docs/design/01-architecture.md)
- Full design: [docs/design](https://github.com/ostrium-labs/loams/blob/dev/docs/design/README.md) (start with the pitch and the architecture)
- Decisions: [decision log](https://github.com/ostrium-labs/loams/blob/dev/docs/design/13-decision-log.md)
- What is open: everything; Loams has no open core ([CONTRIBUTING](https://github.com/ostrium-labs/loams/blob/dev/CONTRIBUTING.md#everything-is-open-source))

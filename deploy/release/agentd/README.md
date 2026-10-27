# agentd/

For the stack's `agentd` service, which runs by default. `config.toml` here is mounted read-only at
`/home/loams/agentd/config.toml` (`LOAMS_AGENTD_CONFIG`); the daemon keeps its
token, lock, discovery file and store beside it, in `/home/loams/agentd`, which
lives in the `agentd-data` volume and is owner-only. This folder is also
mounted read-only at `/etc/loams-agentd`, for files the config names (for
example `[remote]`'s `tls_cert` and `tls_key`).

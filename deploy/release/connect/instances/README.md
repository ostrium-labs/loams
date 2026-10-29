# Connector instances

`loams-fabric connect` (`COMPOSE_PROFILES=connectors`) runs one native connector
instance per `*.yaml` file in this directory. The format and examples are in
`docs/guides/connectors/connect-role.md`. Instance files name secrets; they never
hold them (secrets are files under `../../secrets/connect/<namespace_id>/`).

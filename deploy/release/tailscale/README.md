# tailscale/

`tailscale serve` configurations for the sidecar, picked by `TS_SERVE_MODE`:

- `serve-https.json` (default): `https://<TS_HOSTNAME>.<tailnet>.ts.net/` to the
  engine's tailscale listener on `127.0.0.1:7781` (`LOAMS_TAILSCALE_LISTEN`), with a certificate from Tailscale. Needs
  MagicDNS and HTTPS certificates turned on for the tailnet. Funnel stays off:
  the engine is never on the public internet.
- `serve-http.json` (coming soon, unsupported): HTTP on port 80 of the node to the engine, for Headscale,
  which issues no certificates. A console served over HTTPS cannot
  call an `http://` engine (mixed content), so use it with a console served over
  HTTP from the tailnet or this machine (`--profile console`).

Both are HTTP proxies (not TCP forwards), so `tailscale serve` adds the
`Tailscale-User-Login` headers that `LOAMS_AUTH_MODE=tailscale` relies on.
Only the engine's HTTP API is forwarded. To reach TiDB or a Postgres compute
over the tailnet, add a `TCPForward` entry (for example `"4000": {"TCPForward":
"tidb:4000"}`) after setting their passwords.

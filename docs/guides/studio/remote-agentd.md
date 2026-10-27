# A remote agentd for Studio in the browser

Loams Studio in the browser console works against a `loams-agentd` you run on a machine you control: a dev box, a VM, a container or a workspace sandbox (design [§53](../../design/53-studio-ade.md) §6.3, D909). One remote agentd serves one person (D921). The browser reaches it over TLS on your tailnet with your own short-lived access token, and the daemon's own token never leaves the machine. **Never expose it on the public internet.**

> **Status.** The remote listener, its checks, previews through it and the console's Content-Security-Policy setting ([below](#the-consoles-content-security-policy)) ship in 0.1.0 (SD1 Tasks 7, 26 and 30). The hosted console at `console.loams.dev` is retired (design 78). From 0.1.2 the console is the one your engine serves at `https://loams.<tailnet>.ts.net/ui/`: it reaches an agentd host through **Settings › Agentd hosts** once the engine lists the host in `LOAMS_AGENTD_ORIGINS`, which also lets the host's previews render there. Up to 0.1.1 the browser console was the hosted one, where threads, runs, approvals and diffs worked against a 0.1.0 agentd and previews did not. The 0.1.2 path (the engine-served console with `LOAMS_AGENTD_ORIGINS`) is covered by the engine's tests but has **not been run end to end** against a remote agentd yet. What is **pending**:
> - a sign-in that issues the agentd token, so for now you paste one (IA1);
> - a `.deb`, `.rpm` or pacman package for the daemon, or `cargo install` ([#874](https://github.com/ostrium-labs/loams/issues/874)); from 0.1.2, use the release tarball ([below](#get-the-binary)).

## The short version

The console your engine serves at `/ui/` reaches only the agentd hosts the engine lists in `LOAMS_AGENTD_ORIGINS`, by exact origin. Port 443 keeps the origin short (`https://devbox.<tailnet>.ts.net`); another port works too, as long as the entry names it (`https://devbox.<tailnet>.ts.net:8443`), because a CSP source with no port matches only the scheme's default port.

1. [Get the binary](#get-the-binary) and an empty `config.toml`.
2. Get a certificate for the node with `tailscale cert`, and let the binary bind port 443 ([Port 443 on the tailnet](#port-443-on-the-tailnet)).
3. Set up an [issuer](#an-issuer) and find your `sub`.
4. Run `serve --remote` on the tailnet address, port 443, with `--allow-origin https://loams.<tailnet>.ts.net` (the origin of the console your engine serves), or [run it as a service](#run-it-as-a-service).
5. On the engine, set `LOAMS_AGENTD_ORIGINS=https://devbox.<tailnet>.ts.net` (in the compose stack's `.env`) and restart it ([the console's Content-Security-Policy](#the-consoles-content-security-policy)).
6. In the console at `https://loams.<tailnet>.ts.net/ui/`, open Studio › **Settings** › **Agentd hosts**, add `https://devbox.<tailnet>.ts.net` and paste a token ([In Studio](#in-studio)).

## Get the binary

From 0.1.2, each release attaches `loams-agentd` for Linux as one tarball per architecture, `loams-agentd-<version>-linux-x86_64.tar.gz` and `loams-agentd-<version>-linux-aarch64.tar.gz`. It is not in the `loams` CLI packages (`.deb`, `.rpm`, pacman), and the `loams-agentd-<version>.spdx.json` asset is its SBOM, not the binary. Download the tarball for your machine (`uname -m`) with the release's `SHA256SUMS` and `SHA256SUMS.asc`, check them with the release key ([`signing.md`](../../release/signing.md)), and unpack it:

```sh
v=X.Y.Z                                # the release, without the leading v
t=loams-agentd-$v-linux-$(uname -m)    # x86_64 or aarch64
base=https://github.com/ostrium-labs/loams/releases/download/v$v
curl -fL -O "$base/$t.tar.gz" -O "$base/SHA256SUMS" -O "$base/SHA256SUMS.asc"
gpg --verify SHA256SUMS.asc SHA256SUMS # with the release key imported (signing.md)
sha256sum -c --ignore-missing SHA256SUMS
tar -xzf "$t.tar.gz"
install -D -m 0755 "$t/loams-agentd" ~/.local/bin/loams-agentd
loams-agentd version                   # loams-agentd X.Y.Z
```

The unpacked directory holds `loams-agentd`, `loams`, `SHA256SUMS`, `VERSION` and the licence files. That is the layout `loams-agentd install-runtime --from` reads, the same as Loams Desktop's `resources/bin/`, so the directory can also install the runtime of the [per-user service](#run-it-as-a-service); `loams` is in it because `install-runtime` copies it too. Both binaries are built on Ubuntu 24.04 (glibc 2.39), so they run on Debian 13, Ubuntu 24.04, Fedora 40 and newer.

A release without the tarballs (0.1.0 and 0.1.1 have none) leaves the published image, `ghcr.io/ostrium-labs/loams-agentd` (amd64 and arm64), where the binary is `/usr/local/bin/loams-agentd`:

```sh
podman create --name loams-agentd-copy ghcr.io/ostrium-labs/loams-agentd:0.1.1
podman cp loams-agentd-copy:/usr/local/bin/loams-agentd ~/.local/bin/
podman rm loams-agentd-copy
loams-agentd version        # loams-agentd 0.1.1
```

(`docker create` and `docker cp` work the same way.) The image's binary is linked against glibc 2.39 and nothing else of note, so it runs on the same distributions.

Loams Desktop also bundles it (Linux, and macOS and Windows since 0.1.0), in the app's `resources/bin/`, for example `/opt/Loams Desktop/resources/bin/loams-agentd` on Linux and `/Applications/Loams Desktop.app/Contents/Resources/bin/loams-agentd` on macOS. You can also build it from source: `cargo build --release -p loams-agentd`.

## What `serve --remote` opens

```sh
loams-agentd serve --remote \
  --listen 100.101.102.103:443 \
  --tls-cert ~/.config/loams-agentd/tls/cert.pem --tls-key ~/.config/loams-agentd/tls/key.pem \
  --issuer http://127.0.0.1:8080/realms/loams \
  --audience loams-agentd \
  --subject <your sub> \
  --allow-origin https://loams.<tailnet>.ts.net \
  --config ~/.config/loams-agentd/config.toml
```

Here `100.101.102.103` stands for the machine's tailnet address (`tailscale ip -4`) and the issuer is the [Keycloak recipe](#keycloak-tested) below. `serve` runs the daemon as `run` does (the ndjson listener, the loopback Connect listener and everything else) and adds the remote listener.

**Flags and config:**
- **Every flag is required**, except `--allow-origin`, which you repeat once per console origin. A missing flag exits 4 and names it (`loams-agentd: serve --remote needs --tls-cert`). So does a certificate, key, issuer or origin that can't be used. Nothing has a default that opens the daemon up.
- **`--config` names the daemon's `config.toml`, a file.** It must exist, for `serve`, `run`, `status` and `service` alike, but it may be empty: every key has a default. A directory, or a missing file, exits 4 (`loams-agentd: cannot read config …: Is a directory (os error 21)`). The daemon writes its lock, token, discovery file, store and logs in the directory that holds `config.toml` and makes that directory owner-only, so it must be the daemon's own and writable. `LOAMS_AGENTD_CONFIG` overrides `--config`.
- **The same settings can live in `config.toml`.** Add a `[remote]` table with `enabled = true` and the keys `listen`, `tls_cert`, `tls_key`, `issuer`, `audience`, `subject` and `allow_origins`. Then plain `loams-agentd run` opens the listener too ([example](#run-it-as-a-service)). Without `enabled = true` the listener stays off, and with it every key but `allow_origins` is required. On Windows, use `config.toml`: the service re-runs its worker without the flags.

```sh
mkdir -p ~/.config/loams-agentd && touch ~/.config/loams-agentd/config.toml
```

**What the listener serves:**
- **Only `loams.agentd.v1`**, Connect over HTTP/1.1 and HTTP/2 (ALPN `h2` and `http/1.1`, TLS 1.2 and 1.3), plus the [preview paths](#previews). The ndjson wire, `Hello` and `Shutdown` stay on the loopback listener, which still refuses browsers (D788). `agentd.json` doesn't publish the remote port.
- **Each request is checked in this order** before any handler runs:
  1. **Origin.** The request has no `Origin` header (curl, a CLI), or its origin is exactly one of yours. Any other origin gets a 403 `origin_not_allowed` with no CORS headers.
  2. **Preflight.** An `OPTIONS` preflight from one of your origins is answered here, before authentication.
  3. **Token.** `Authorization: Bearer <access token>`, a JWT signed by the issuer with RS256 or EdDSA (HMAC and `none` are refused). The daemon finds the issuer's keys through discovery, caches them, and refetches at most once a minute for an unknown key. It checks `iss`, `exp` and `nbf` with 60 s of leeway, and requires `aud` to contain the audience. A token that doesn't verify gets a 401 `unauthenticated`. If no signing keys can be fetched and none are cached, the answer is a 503.
  4. **Subject.** `sub` must equal `--subject`. A valid token for anyone else gets a 403 `forbidden_role`.
  5. **Role.** Your browser session runs in the role `studio`. It can call every Studio method except the harness installers and updaters (`InstallHarness`, `CancelInstall`, `ApplyHarnessUpdate`, `CancelHarnessUpdate`) and the agent account logins and switches (`StartAgentLogin` and its siblings, `ActivateAgentAccount`, `ForgetAgentAccount`), Q808. Do those on the machine itself.
- **CORS answers only your origins.** It allows the Connect headers and `Authorization`, exposes `grpc-status`, `grpc-message` and `grpc-status-details-bin`, caches preflights for 7200 s, and never sends `Access-Control-Allow-Credentials`. The console sends a bearer token, never a cookie. An origin must be `https://host[:port]` (`http://` only on loopback), never `*`, `null` or a path.
- **Harnesses** are the CLIs installed on the machine for the daemon's user (Claude Code, Codex, opencode…): `ListHarnesses` finds them on the login shell's `PATH`.

A quick check from another machine on the tailnet:

```sh
# A preflight from your console's origin: 204 and the CORS headers.
curl -s -D - -o /dev/null -X OPTIONS \
  -H 'Origin: https://loams.<tailnet>.ts.net' -H 'Access-Control-Request-Method: POST' \
  https://devbox.<tailnet>.ts.net/loams.agentd.v1.StudioService/GetHost
# Another origin: 403, no CORS headers. No token: 401 unauthenticated.
curl -s -X POST -H 'Content-Type: application/json' -d '{}' \
  https://devbox.<tailnet>.ts.net/loams.agentd.v1.StudioService/GetHost
```

These answers, a valid-token call through Keycloak and the then-hosted console's whole handshake were checked against the 0.1.0 image's binary on 2026-10-19 and 20 ([#872](https://github.com/ostrium-labs/loams/issues/872)).

## Port 443 on the tailnet

The default (Q804) is **a tailnet** (§43). Listen on the machine's tailnet address, or on `0.0.0.0` with the host firewall closed to everything but the tailnet. Don't forward the port on a router or open it to the internet: an agent the daemon runs can do anything the daemon's user can. An outbound tunnel through the Loams gateway is an open question (Q804); SD1 doesn't build one.

**The certificate.** Turn on MagicDNS and HTTPS certificates for the tailnet (in the Tailscale admin console), then get one for the node and give the files to the daemon's user:

```sh
mkdir -p ~/.config/loams-agentd/tls && cd ~/.config/loams-agentd/tls
sudo tailscale cert --cert-file cert.pem --key-file key.pem devbox.<tailnet>.ts.net
sudo chown "$USER": cert.pem key.pem && chmod 600 key.pem
```

The certificate is a Let's Encrypt one and lasts 90 days. The daemon reads it at start, so run `tailscale cert` again before it expires and restart the daemon.

**Port 443.** A port below 1024 needs `CAP_NET_BIND_SERVICE`. Give it to the binary:

```sh
sudo setcap cap_net_bind_service=+ep ~/.local/bin/loams-agentd
getcap ~/.local/bin/loams-agentd      # …/loams-agentd cap_net_bind_service=ep
```

Do it again whenever you replace the binary; a file system mounted `nosuid` ignores it. Instead of `setcap`, a system service can grant it: a system unit with `User=<you>` and `AmbientCapabilities=CAP_NET_BIND_SERVICE`. Without either, the daemon exits when it can't bind the remote listener.

**Other names and ports.** Any console that takes `LOAMS_AGENTD_ORIGINS` ([below](#the-consoles-content-security-policy)) can reach other ports and names, and a self-signed certificate works there once the browser trusts it (open the host's URL once and accept it, or install your own CA). Note its SHA-256 fingerprint when you register the host, to compare with what the browser shows: `openssl x509 -in cert.pem -noout -fingerprint -sha256`.

## Run it as a service

A systemd **user** unit keeps the daemon running, and linger makes it start at boot and survive logout. Put the remote settings in `config.toml` rather than on the command line:

```toml
# ~/.config/loams-agentd/config.toml
[remote]
enabled = true
listen = "100.101.102.103:443"        # tailscale ip -4
tls_cert = "/home/you/.config/loams-agentd/tls/cert.pem"
tls_key = "/home/you/.config/loams-agentd/tls/key.pem"
issuer = "http://127.0.0.1:8080/realms/loams"
audience = "loams-agentd"
subject = "<your sub>"
allow_origins = ["https://loams.<tailnet>.ts.net"]
```

```ini
# ~/.config/systemd/user/loams-agentd-remote.service
[Unit]
Description=Loams agent daemon for Studio in the browser (tailnet only)

[Service]
Type=exec
ExecStart=%h/.local/bin/loams-agentd run --service --config %h/.config/loams-agentd/config.toml
Restart=on-failure
RestartSec=5
# 3: another daemon holds this directory; 4: the config is invalid. Neither heals on a restart.
RestartPreventExitStatus=3 4
KillMode=mixed

[Install]
WantedBy=default.target
```

```sh
systemctl --user daemon-reload
systemctl --user enable --now loams-agentd-remote.service
loginctl enable-linger "$USER"        # or: loams-agentd service enable-linger
journalctl --user -u loams-agentd-remote -f
loams-agentd status --config ~/.config/loams-agentd/config.toml
```

The unit is named `loams-agentd-remote` so it doesn't collide with the one Loams Desktop installs. If tailscaled isn't up yet at boot, the bind fails and systemd retries.

**`loams-agentd service`** (`install`, `uninstall`, `start`, `stop`, `status`, `enable-linger`, `disable-linger`) is how Loams Desktop installs its per-user service: on Linux it writes `~/.config/systemd/user/loams-agentd.service`, which runs `<daemon dir>/runtime/current/loams-agentd run --service --config <config>`, so `[remote]` in that `config.toml` applies to it too. It needs that runtime first: `loams-agentd install-runtime --from <dir> --to <daemon dir>/runtime` copies `loams-agentd` and `loams` from `<dir>` and checks them against the `SHA256SUMS` and `VERSION` beside them, which is the layout of Loams Desktop's `resources/bin/` and of the [release tarball](#get-the-binary): `"$t/loams-agentd" install-runtime --from "$t" --to ~/.config/loams-agentd/runtime`. On a headless host without Desktop, the unit above is simpler. If you do use `service install`, run `setcap` on `runtime/current/loams-agentd`, and again after each `install-runtime`.

## An issuer

The daemon accepts tokens from one OIDC issuer, for one audience and one subject. The issuer must be `https://`, or `http://` on a loopback host (`127.0.0.1`, `::1`, `localhost`), which is the easy path for one person: run the issuer on the same machine as the daemon. The daemon reads `<issuer>/.well-known/openid-configuration`, and the `issuer` it names must be `--issuer` (a trailing `/` doesn't matter).

### Keycloak (tested)

This recipe ran end to end against the 0.1.0 daemon, with Keycloak on `127.0.0.1:8080` on the daemon's machine:

- **A realm**, `loams`. The issuer is `http://127.0.0.1:8080/realms/loams`. Request tokens through the same host name: Keycloak puts the host you asked in `iss`, so a token fetched through `localhost` fails against an issuer on `127.0.0.1`.
- **A client**, `loams-agentd`: confidential (client authentication on), **direct access grants** on, standard flow off.
- **An audience mapper.** Keycloak doesn't put the client id in `aud` on its own. In the client's dedicated scope (`loams-agentd-dedicated`), add a mapper of type **Audience** (`oidc-audience-mapper`) with **Included Client Audience** `loams-agentd` and **Add to access token** on.
- **A user** for you. Your subject is that user's `sub`, its **ID** in the admin console's user page. Pass it as `--subject`.
- **A longer access token lifespan.** The console keeps the pasted token in the tab's memory and can't refresh it, so Keycloak's default of 5 minutes means pasting a new one every 5 minutes. Raise the realm's **Access Token Lifespan** (Realm settings › Tokens, `accessTokenLifespan`), for example to a working day. Keycloak doesn't issue an access token that outlives the SSO session, so raise **SSO Session Idle** and **SSO Session Max** with it if tokens still expire early.
- Keep the realm's default RS256 signing key.

Fetch a token on the machine (the password grant is what direct access grants turn on), then paste it into the console:

```sh
curl -s http://127.0.0.1:8080/realms/loams/protocol/openid-connect/token \
  -d grant_type=password -d client_id=loams-agentd -d client_secret="$CLIENT_SECRET" \
  -d username=you -d password="$PASSWORD" -d scope=openid | jq -r .access_token
```

### Zitadel (planned)

Zitadel at `auth.loams.dev` is planned (IA1, [design 76](../../design/76-identity-and-authorization.md); [`zitadel.md`](../../platform/deploy/zitadel.md)). Until a recipe here is tested against it, use any OIDC issuer that meets the rules above, such as the Keycloak recipe.

**How the console gets the token.** It will get an access token for the audience `loams-agentd` through its own sign-in at Zitadel once IA1 lands (design 76 §10). Until then, **Settings › Agentd hosts** asks you to paste a token per host, under a "Development sign-in" warning. The token stays in that tab's memory and is gone when you reload.

## A container recipe

The release builds an image, `ghcr.io/ostrium-labs/loams-agentd`, from [`deploy/release/Containerfile.loams-agentd`](../../../deploy/release/Containerfile.loams-agentd) (`deploy/release/build.sh loams-agentd` builds it locally). Its entry point is `loams-agentd`, it runs as uid 10001, and it sets `LOAMS_AGENTD_CONFIG=/home/loams/agentd/config.toml`, which overrides `--config`. The daemon keeps its token, lock and store beside `config.toml` and makes that directory owner-only, so it must be the daemon's own and writable: `/home/loams/agentd` is, in the image (SD1 Task 30). **It holds no harness CLIs:** build your own image from it with the agents you use (Claude Code, Codex, opencode…) installed.

Run it with only your project directories mounted (§53 §8.2). Worktrees keep agents' edits apart, but they don't contain processes: an agent with a shell can read anything the daemon's user can.

```sh
# /home/loams (a volume) holds agentd/, the daemon's directory; the daemon needs a
# config.toml there (it may be empty), mounted read-only on its own. The TLS files are
# mounted read-only somewhere else. Publish the port on the tailnet address only.
podman run -d --name loams-agentd \
  -v loams-agentd-home:/home/loams \
  -v ./config.toml:/home/loams/agentd/config.toml:ro \
  -v ~/src:/home/loams/src \
  -v ./tls:/etc/loams-agentd-tls:ro \
  -p 100.101.102.103:443:8443 \
  <your image, FROM ghcr.io/ostrium-labs/loams-agentd, with your harness CLIs> \
  serve --remote --listen 0.0.0.0:8443 \
    --tls-cert /etc/loams-agentd-tls/cert.pem --tls-key /etc/loams-agentd-tls/key.pem \
    --issuer https://auth.example.com/application/o/loams-agentd/ \
    --audience loams-agentd --subject <your sub> \
    --allow-origin https://loams.<tailnet>.ts.net
```

Inside the container the daemon listens on 8443; the host publishes it on 443. Rootless Podman can publish a port below 1024 only when `net.ipv4.ip_unprivileged_port_start` allows it. The issuer must be reachable from inside the container, so a loopback issuer on the host doesn't work here.

**Unverified:** this recipe has not been run end to end against the published image. `deploy/release/compose.yaml`'s optional `agentd` service (`--profile agentd`) runs the same image with `run`, not `serve --remote`.

## In Studio

1. Open Studio, then **Settings** (Mod+,). In the browser the page has an **Agentd hosts** card. Add the host's name and its `https://` URL (`http://` only for `localhost`), for example `https://devbox.<tailnet>.ts.net`, and optionally the certificate fingerprint as a note. The list is kept in this browser, under `loams.studio.hosts`; it doesn't follow you to another browser.
2. Paste an access token for the host.
3. Studio uses the first host you add; **Use** switches hosts. A link can name a host as `?host=<id>`, by its registered id only; a URL in `?host=` is ignored.

Without a host, or without a token for the selected host, Studio's home shows **Connect an agentd host**.

## The console's Content-Security-Policy

The browser console only fetches from and frames the origins its Content-Security-Policy names. A deployment lists its agentd hosts in **`LOAMS_AGENTD_ORIGINS`** (SD1 Task 30, ruling T30-3): origins separated by spaces or commas, each an exact `https://host[:port]` (`http://` only on `localhost`), with no wildcard, path or credentials. The engine-served console (`/ui/`, from 0.1.2) and `deploy/stage.mjs` add them to both `connect-src` (the Connect calls) and `frame-src` (the previews). Each host must also name the console's origin in its `--allow-origin`.

For the console your engine serves, set the variable on the engine. In the compose stack, put it in `.env`, then `docker compose up -d loams`:

```sh
# .env
LOAMS_AGENTD_ORIGINS=https://devbox.<tailnet>.ts.net,https://vm.<tailnet>.ts.net:8443
```

The engine refuses to start on an entry that is not an exact origin (`*`, `null`, a path or credentials), and names the variable. For a console you stage yourself:

```sh
LOAMS_AGENTD_ORIGINS="https://devbox.example.com:8443 https://vm.example.com" \
  node web/apps/console/deploy/stage.mjs
```

| Console | Reaches an agentd host when |
|---|---|
| The console your engine serves at `/ui/` (0.1.2 and later) | the host's origin is in the engine's `LOAMS_AGENTD_ORIGINS`: the engine sends the build's policy as a header with those origins in `connect-src` and `frame-src`, so Connect calls and previews work |
| A console you deploy (`deploy/stage.mjs`, Cloudflare) | its origin is in `LOAMS_AGENTD_ORIGINS`, which also lets its previews frame; a `*.ts.net` name on port 443 is in `connect-src` already |
| Vercel (`vercel.json`) | its origin is in `vercel.json`'s `connect-src` and `frame-src` (Vercel headers are static; the stage step fails when `LOAMS_AGENTD_ORIGINS` names one they lack) |
| The production build served as static files, without `stage.mjs` or the engine | never: its meta policy is the build's `'self'` |
| The dev server (`pnpm dev`) | always (no policy) |
| The hosted console, `console.loams.dev` (retired) | up to 0.1.1, a `*.ts.net` name on port 443, without previews |

The desktop is not affected: it reaches its local daemon through Electron main, and its preview is a native view.

## Previews

**Previews render in a console whose `frame-src` lists your host:** the console your engine serves at `/ui/` from 0.1.2, or one you deploy, with the host in `LOAMS_AGENTD_ORIGINS`. Without that, the browser refuses to frame the host's preview; the daemon side below still works. Then forward the preview proxy's port instead (`ssh -N -L 7331:localhost:7331 <host>`) and open the preview's `http://<name>.<project>.localhost:7331` URL in a tab. See [Preview](preview.md).

Dev-server previews work through the remote listener (Q806, SD1 Task 26). The daemon's own preview proxy listens on loopback only (`http://<name>.<project>.localhost:7331`), so the remote listener forwards `/preview/…` to it:

- **The console asks for a ticket.** It `POST`s `/preview/-/ticket` with its bearer token, from one of your origins, with the body `{"preview": "<name>.<project>"}`. The answer is a path `/preview/pt_<64 hex>/`: a random capability for that one preview, held in memory (at most 64), lasting until the token expires and at most 8 hours. The console frames that path. A frame cannot send an `Authorization` header, and the listener sets no cookies.
- **A CLI or curl** can call `/preview/<name>.<project>/…` with the bearer token instead.
- **Every preview answer** carries `Content-Security-Policy: frame-ancestors <your --allow-origin list>` (`'none'` without one), refusals included, and `Referrer-Policy: same-origin`, so a ticket never leaves in a `Referer`. The token is never forwarded to the dev server, and the daemon never logs the path.
- **The prefix is stripped** (`/preview/<x>/src/main.tsx` reaches the dev server as `/src/main.tsx`, with `X-Forwarded-Prefix`), and a `Location` header is moved back under it. An app that uses root-relative URLs (`/assets/…`) needs its dev server's base path set to the prefix, as behind any path proxy. HMR WebSockets pass through.
- **Cookies a dev server sets** land on the agentd's origin, where every preview of the same daemon can see them. That is acceptable for one person's daemon (D921), and a reason not to share one.

If the daemon answers the ticket request with 404, 501 or 503 (an older daemon, or previews off), the Preview tab shows how to forward the preview proxy's port instead.

## Approvals

A remote daemon can be reached over the network, so with the remote listener on, a thread that is given no mode is **supervised**: every tool call waits for you (`[approvals] default_mode` overrides this). Your browser session can answer approvals and set a thread's mode, since the `studio` role includes the approval methods. An approval nobody answers is denied after `[approvals] timeout_secs` (1800 s). See [Approvals](approvals.md).

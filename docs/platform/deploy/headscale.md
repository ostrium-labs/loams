# Deploying Headscale for the Loams private network

> **Imported on 2026-10-19.** Written in the former private `ostrium-labs/loams-platform` repository; open source (Apache-2.0) in the monorepo since the owner's order of 2026-10-19. "This repository", "proprietary" and "private" below describe that repository and the open-core boundary of the time, which no longer holds; see [the platform docs index](../README.md).

> **Alternative provider.** Since 2026-10-02 the default tailnet is hosted Tailscale
> (bring your own account), and Headscale is the self-hosted alternative selected
> by configuration only. Start at [`tailnet.md`](tailnet.md). This page is the
> Headscale procedure.

Owner runbook. Headscale (the open-source, self-hosted Tailscale control
server) is the **private network** of the hosted Loams service: SSH and admin
access with no public ports, k3s, TiKV and WeSQL traffic across sites, backups
over private paths, the `loam-bench` CI runner, and Zitadel's admin
console once IA1 deploys it ([`zitadel.md`](zitadel.md)). Owner ruling 2026-10-02: "you can use Headscale if you want, for
added security and unified handling." The design is OSS
[`docs/design/43-private-networking.md`](https://github.com/ostrium-labs/loams/blob/dev/docs/design/43-private-networking.md);
this page is the operating procedure.

Everything is in [`deploy/tailnet/headscale/`](../../../deploy/tailnet/headscale):

| Path | What |
|---|---|
| `compose/` | Target A: Headscale + Caddy + encrypted nightly backup, on one host |
| `kustomization.yaml`, `k8s/` | Target B: the same three containers in one k3s pod |
| `config/config.yaml` | Headscale configuration (OIDC to Zitadel at `auth.loams.dev`, embedded DERP, MagicDNS) |
| `policy/` | The seeded checker (`check.sh`) for the shared policy `../policy/policy.hujson` |
| `check-config.sh` | `headscale configtest` against `config/config.yaml` |

## 0. Facts (checked 2026-10-02)

- **Headscale v0.29.4**, released 2026-09-23, BSD-3-Clause. Image
  `ghcr.io/juanfont/headscale:v0.29.4@sha256:8833f828b414c0907b7e5c71da76473216fe17cce0818a166b536ec552c0903f`
  (same digest on `docker.io/headscale/headscale:v0.29.4`). Minimum Tailscale
  client: v1.80.0. Current Tailscale client: v1.102.5 (2026-09-29).
- **No official Helm chart.** The project documents Docker and Podman only;
  community charts exist and are not affiliated, so Target B is plain manifests
  pinned by digest.
- **Headscale cannot sit behind Cloudflare's proxy or Tunnel.** Its own
  documentation: "Running Headscale behind a Cloudflare Proxy or Cloudflare
  Tunnel is not supported and will not work as Cloudflare does not support
  WebSocket POSTs as required by the Tailscale protocol." The control protocol
  upgrades a POST with `Upgrade: tailscale-control-protocol`. So
  `headscale.loams.dev` is a **DNS-only** record (grey cloud) pointing at the
  host, and the host opens 80/tcp, 443/tcp and 3478/udp. `auth.loams.dev` and
  the site stay behind Cloudflare.
- **OIDC groups cannot be used in policy rules.** IdP groups can gate who may
  sign in (`oidc.allowed_groups`), but `group:ops` in the policy is a list of
  emails kept in Git. Tags are owned by `group:ops`.
- **A tagged pre-auth key fixes the node's tags; do not also pass
  `--advertise-tags`.** Checked 2026-10-02: `tailscale up --authkey <tagged key>
  --advertise-tags=<tag>` is refused ("requested tags ... are invalid or not
  permitted"); without the flag the node registers tagged. (Tailscale accepts both.)
- **Tagged nodes do not expire.** Personal devices expire after 90 days
  (`node.expiry`); servers and CI do not. Retire servers with
  `headscale nodes delete`; CI nodes are ephemeral.
- **Postgres is "highly discouraged"** by the project (legacy only); the
  supported path is SQLite, one writer, one replica. Backups are the safety net.
- **A policy `tests` entry that names a tag or user with no node fails.** Checked
  2026-10-02 against v0.29.4 with this policy and no nodes: at boot Headscale logs
  "policy tests failed at boot; server starting anyway", and a `SIGHUP` reload
  with the same policy is refused ("reloading policy ... destination resolved to
  no IP addresses"). `policy/check.sh` seeds one node per tag for that reason; on a
  live server reload only after a node exists for every tag the tests name, or trim
  the tests you do not yet have nodes for.
- **A failing policy test does not stop Headscale starting** ("policy tests
  failed at boot; server starting anyway"). `policy/check.sh` is therefore a
  required check on every change.
- **Not supported by Headscale:** Funnel, Serve, network flow logs, device
  posture. There is no per-event audit log: see section 11.

## 1. What goes where

| Traffic | Network |
|---|---|
| `loams.dev` (with `/cloud`), `auth.loams.dev` (people, browsers, OIDC; `console.loams.dev` is retired, D1705) | Cloudflare (Workers, Tunnel). Public |
| SSH, `kubectl`, Zitadel's admin console, k3s, TiKV, WeSQL, S3 backups, bench runner, BYOC agents to the control plane | The Headscale tailnet. Private |
| Tailnet sign-in and coordination, DERP relay | `headscale.loams.dev`, DNS-only, public by necessity |

## 2. Prerequisites (owner actions)

1. **A host** with a public IPv4 (and IPv6 if available), Docker Engine 25+ with
   the Compose plugin, **1 vCPU / 1 GB RAM / 10 GB disk** is plenty (the
   Headscale process is small; this is a coordinator, not a data path, except
   for DERP relaying). It may share the `auth.loams.dev` host, but a separate
   small VM is better: a different failure domain for the thing everything else
   depends on. Open exactly 80/tcp, 443/tcp, 3478/udp.
2. **DNS:** `headscale.loams.dev` A (and AAAA) to the host, **DNS only**.
3. **An object-store bucket** for backups (a shared bucket works; use the
   prefix `headscale`).
4. **A backup keypair**: `age-keygen -o headscale-backup.key` on a trusted
   machine. Put the public line (`age1...`) in `.env`. Keep the private file
   offline (password manager, safe). It is the only way to read a backup.
5. **The first admin account** (section 5).

## 3. The OIDC client

Headscale signs people in at Zitadel: issuer `https://auth.loams.dev`, client
`headscale`, redirect URI `https://headscale.loams.dev/oidc/callback`. The
secret is `HEADSCALE_OIDC_CLIENT_SECRET` in Headscale's environment (compose
`.env`, or the k3s Secret). Headscale does not support JWE (encrypted ID tokens).

**Status: planned (IA1).** Zitadel is not deployed yet, and the `headscale`
application is created by IA1's setup for `auth.loams.dev`
([`zitadel.md`](zitadel.md)). Which claim gates sign-in
(`oidc.allowed_groups` in `config/config.yaml`) is settled with that task. Until
then, register devices with pre-auth keys (section 5, step 2): Headscale starts
without the IdP (`only_start_if_oidc_is_available: false`).

## 4. Start

### Target A: compose

```sh
cd deploy/tailnet/headscale
cp compose/.env.example compose/.env && chmod 600 compose/.env   # fill it in
sed -i 's/admin@loams.dev/YOU@your-domain/' ../policy/policy.hujson  # your email
HEADSCALE_BIN=... ./policy/check.sh && ./check-config.sh          # or use the container (needs Docker)
cd compose && docker compose up -d --build
docker compose logs -f headscale caddy      # wait for the certificate
curl -sS https://headscale.loams.dev/health # {"status":"pass"}
```

`policy.hujson` lists operators by email in `group:ops` and in its `tests`;
replace the placeholder **in all of its occurrences** and keep the list equal to
the operators the IdP lets sign in (section 3).

### Target B: k3s

```sh
kubectl apply -f deploy/tailnet/headscale/k8s/namespace.yaml
# create the two Secrets (commands in k8s/secrets.example.yaml)
kubectl kustomize --load-restrictor=LoadRestrictionsNone deploy/tailnet/headscale | kubectl apply -f -
```

Backups run as the `headscale-backup` CronJob (03:40 UTC), not as a sidecar, so a
failing package mirror or bucket cannot take the control plane out of service.
It fetches `sqlite`, `age` and `tar` with `apk` when it starts (it has no image
of its own); the compose image is the preferred form. Caddy's certificates live
on their own volume so a reschedule does not burn Let's Encrypt's limits.

## 5. The first admin

1. Open `https://headscale.loams.dev/` and sign in from a laptop (section 6):
   `tailscale up --login-server https://headscale.loams.dev`. The browser goes
   to `auth.loams.dev`; the first login creates the Headscale user (named from
   your username or email at the IdP).
2. Until Zitadel is deployed (IA1), or while bootstrapping, use the CLI path instead:

   ```sh
   docker compose exec headscale headscale users create bootstrap
   docker compose exec headscale headscale users list         # note the ID
   docker compose exec headscale headscale preauthkeys create --user <ID> --expiration 1h
   ```

   Join with `tailscale up --login-server ... --authkey <key>`. Delete the user
   and its nodes once OIDC works.
3. Confirm: `tailscale status` shows the node; on the server
   `headscale nodes list` lists it with your user.

## 6. Joining devices

Commands use the CLI inside the container. From any directory, with `REPO` set
to the checkout:
`alias hs='docker compose -f "$REPO/deploy/tailnet/headscale/compose/docker-compose.yml" --env-file "$REPO/deploy/tailnet/headscale/compose/.env" exec headscale headscale'`.
On k3s: `alias hs='kubectl -n headscale exec deploy/headscale -c headscale -- headscale'`.

### An admin laptop or desktop (Linux, macOS, Windows)

```sh
tailscale up --login-server https://headscale.loams.dev
```

macOS GUI: Option-click the menu icon, **Debug -> Custom Login Server -> Add
Account**. Windows: the same command from a terminal (the instance also serves
setup instructions at `/windows` and `/apple`). Sign in at `auth.loams.dev`
(or use a pre-auth key until it runs Zitadel). The
device is owned by you, is untagged, expires in 90 days, and can reach what the
policy grants `group:ops`.

### A phone or tablet (iOS, Android)

Install the official Tailscale app. iOS: account icon -> **Log in** -> menu ->
**Use custom coordination server**. Android: Settings -> Accounts -> menu ->
**Use an alternate server**. Enter `https://headscale.loams.dev`. Loams' own mobile
apps do not embed a tailnet client: they use the phone's VPN like any app
(design 43 section 7).

### A server (the auth host, Headscale host, TiKV, WeSQL, object store)

On the Headscale host, create a **tagged, single-use** key valid for an hour:

```sh
hs preauthkeys create --tags tag:zitadel --expiration 1h
```

On the server (Tailscale from the distribution or `tailscale.com/install.sh`,
BSD-3-Clause):

```sh
tailscale up --login-server https://headscale.loams.dev --authkey <key> --hostname auth-1
# once SSH over the tailnet works from another device, close public SSH:
sudo ufw allow in on tailscale0 to any port 22 proto tcp
sudo ufw delete allow 22/tcp
```

The node is named `auth-1.net.loams.dev`. Tags are fixed by the key; the policy
decides what the tag can reach. Use one tag per role (`tag:k3s`, `tag:tikv`,
`tag:wesql`, `tag:objstore`, `tag:headscale`, `tag:zitadel`).

### k3s nodes

Join each node as `tag:k3s` as above, then start k3s on the tailnet address:

```sh
k3s server --node-ip "$(tailscale ip -4)" --node-external-ip "$(tailscale ip -4)" \
  --flannel-iface tailscale0 --advertise-address "$(tailscale ip -4)" \
  --tls-san "$(hostname).net.loams.dev"
```

Agents use `--node-ip` and `--flannel-iface tailscale0` and join
`https://<server>.net.loams.dev:6443`. The policy grants the ports k3s needs
between `tag:k3s` nodes (6443, 10250, 2379-2380, flannel VXLAN 8472/udp,
WireGuard 51820-51821/udp). Close 6443 and 10250 on the public interface.

### The bench runner (`loam-bench`)

Join as `tag:bench`. The GitHub Actions runner connects outbound to GitHub as
before; the tailnet is for operator SSH and for the runner to reach the private
object store. CI jobs that need the tailnet get a **15-minute, single-use,
ephemeral** key from the workflow's secret store:

```sh
hs preauthkeys create --tags tag:ci --ephemeral --expiration 15m
```

An ephemeral node is deleted after 30 minutes without contact
(`node.ephemeral.inactivity_timeout`).

### BYOC tenants

Customer clusters never get an operator key. The control plane (OSS plan NET1)
creates a tenant tag and a single-use, tagged key per enrolment; the policy
section for the tenant is generated. By hand, for a trial:

```sh
hs preauthkeys create --tags tag:byoc-acme --expiration 1h   # tag must exist in the policy
```

## 7. The host joins its own tailnet

Join the Headscale host as `tag:headscale` (section 6), then set
`ADMIN_BIND=$(tailscale ip -4)` in `compose/.env` and `docker compose up -d`.
The management API (`/api`, used by automation with an API key) is now on
`http://<tailnet-ip>:8081` only. It is never on the public name: Caddy answers
`404` for `/api`, `/metrics`, `/debug` and `/swagger` there. Create API keys
with `hs apikeys create --expiration 90d` and store them in the secret manager.

## 8. Zitadel's admin console on the tailnet

**Planned (IA1).** Zitadel's admin console and management APIs are never public
(D1617, carrying D596's rule over): only the OIDC endpoints and the login UI are
on `auth.loams.dev`. The auth host joins as `tag:zitadel`, and how the admin
console is published on the tailnet comes with IA1's `auth.loams.dev` task
([`zitadel.md`](zitadel.md)).

## 9. Policy changes

Edit `../policy/policy.hujson` (shared with Tailscale, see [`tailnet.md`](tailnet.md)) in a pull request. The required check is
`policy/check.sh`: Headscale's own `policy check` run against a scratch database
seeded with one node per tag and operator, so the `tests` block is evaluated.
After merge, on the host: `git pull`, then reload without a restart:

```sh
docker compose -f deploy/tailnet/headscale/compose/docker-compose.yml kill -s HUP headscale
docker compose logs --tail 20 headscale     # confirm the result of policy processing
```

Adding an operator: let them sign in at the IdP (section 3), add their email to `group:ops` **and** add a test line, check,
merge, reload. Removing one: remove their sign-in at the IdP, remove the email, reload,
then `hs nodes list` and `hs nodes delete` / `expire` their nodes.

## 10. Backups, restore and recovery

Nightly at 03:40 UTC the `backup` container writes one object
`headscale-<time>.tar.age` containing a consistent SQLite copy, the Noise and
DERP private keys and the policy, **encrypted to the offline age key**, and
prunes objects older than 30 days. A heartbeat URL (optional) pages if it stops.

Take one now: `docker compose exec backup /usr/local/bin/backup.sh now` (a stale lock after a killed run: `docker compose exec backup rm -rf /scratch/lock`).

**Restore drill (do it before relying on this, and quarterly):**

```sh
mkdir /tmp/hs-restore && cd deploy/tailnet/headscale/compose
docker compose run --rm -v /tmp/hs-restore:/out -v /secure/headscale-backup.key:/id:ro \
  backup /usr/local/bin/restore.sh headscale-<time>.tar.age /id /out
```

Real recovery of a lost host: build a new host, `docker compose up -d` once to
create the volume, stop it, copy `db.sqlite`, `noise_private.key` and
`derp_server_private.key` from the restore into the `lib` volume, start. The
**Noise key is the server's identity**: with it restored, nodes reconnect
without re-registering. Move the DNS record to the new host's address.

| Failure | Effect | Action |
|---|---|---|
| Headscale process or host down | **Existing connections keep working** (WireGuard is peer to peer and does not need the control server). New logins, new nodes, key renewals, policy and route changes, and new peer discovery fail; MagicDNS keeps resolving known names; relayed (DERP) connections drop with the host | Restart; restore from backup if the volume is lost. Target: back within one hour |
| IdP down | New OIDC sign-ins fail; existing nodes unaffected; pre-auth key registrations still work. Headscale starts without the IdP (`only_start_if_oidc_is_available: false`) and falls back to CLI registration | Fix the IdP; use `hs auth register --user <u> --auth-id <id>` for an urgent device |
| DERP down (same host) | Peers that cannot reach each other directly lose connectivity; direct paths remain | Same as host down. A second DERP region on another provider is a documented follow-up (design 43 Q586) |
| Bad policy pushed | Headscale keeps running on a failing test ("starting anyway"); rules may lock operators out | Revert the commit, `kill -s HUP`. Break-glass: provider console, edit the file on the host, reload |
| Node key stolen (server) | Tagged nodes do not expire | `hs nodes delete -i <id>` and rotate whatever it held; investigate |
| Operator laptop lost | Device stays valid until it expires | `hs nodes expire -i <id>`, remove their sign-in at the IdP, reload policy |
| Headscale host compromised | It is the control point: an attacker can add nodes and rewrite policy | Rotate: new host, new Noise key (all nodes re-register), new OIDC secret, review all nodes. This is why it holds no data and no other secrets |

## 11. Audit and monitoring

- Headscale has no audit-event stream. Evidence is: the container's JSON logs
  (registrations, expiries, policy reloads) shipped to the log pipeline; Git
  history of `deploy/tailnet/policy/policy.hujson` (every access change is a reviewed commit);
  the IdP's event log for each sign-in (Zitadel's events once it runs); and a nightly inventory
  (`hs nodes list -o json` compared with the previous day's, alert on a new tag
  or an untagged server).
- Metrics: `http://127.0.0.1:9090/metrics` inside the container (not published);
  scrape from `tag:k3s` over the tailnet if you publish it on the tailnet
  address. Alert on the process being down, on `headscale health`, and on the
  certificate (Caddy renews automatically; watch the expiry).

## 12. Upgrades

1. Read the release notes; check the minimum client version.
2. Take a backup (`backup.sh now`).
3. Change the three places that name the version and digest (compose, k8s, and
   `policy/check.sh`/`check-config.sh`), run `./check-config.sh` and
   `./policy/check.sh`, then `docker compose pull && docker compose up -d`.
4. The database migrates on start. Do not skip minor versions.

## 13. What was verified, and what was not

Verified on 2026-10-02 (Podman 6.1.1 with `docker-compose` 5.2.0, the pinned
images, Headscale v0.29.4): `policy/check.sh` with the container and with the
release binary (passes; a flipped test fails with the denied pair named);
`check-config.sh`; `docker compose config`; `caddy validate`; the Headscale
service healthy under `read_only` with the shipped config, and falling back to
CLI registration when the issuer is unreachable (log: "failed to set up OIDC
provider, falling back to CLI based authentication"); the backup image built;
`backup.sh now` and `restore.sh` round trip with an age key (the restored
database held the user created before the backup, plus both private keys and
the policy); and `kubectl kustomize` for Target B. The 2026-10-02 sign-in checks were
against the identity provider of the time, which D1600 removed; they are redone
against Zitadel with IA1.

**Not verified here (owner or first deploy):** the Let's Encrypt certificate and
real DNS; a full Headscale to Zitadel sign-in (needs Zitadel at `auth.loams.dev`); a
client login on a phone, macOS and Windows (the platform steps come from
Headscale's documentation); embedded DERP through Caddy with real clients; k3s on the
tailnet interface; Target B end to end. Expect the log line "listening without
TLS but ServerURL does not start with http://": Headscale is behind Caddy, and
it is harmless.

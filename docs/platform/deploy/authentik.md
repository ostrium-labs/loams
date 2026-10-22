# Deploying Authentik for auth.loams.dev

> **Imported on 2026-10-19.** Written in the former private `ostrium-labs/loams-platform` repository; open source (Apache-2.0) in the monorepo since the owner's order of 2026-10-19. "This repository", "proprietary" and "private" below describe that repository and the open-core boundary of the time, which no longer holds; see [the platform docs index](../README.md).

Owner runbook. Authentik (open-source edition) is the identity provider for
loams.dev (ruling of 2026-10-02: "remove Clerk; we deploy Authentik and use
it"). It runs at **https://auth.loams.dev**. The site (`ostrium-labs/loams-site`,
Next.js on Cloudflare Workers) signs in through it with Auth.js.

Everything is in [`deploy/authentik/`](../../../deploy/authentik):

| Path | What |
|---|---|
| `compose/` | Target A: one VM, Docker Compose, `cloudflared`, nightly backups |
| `helm/` + `kustomization.yaml` | Target B: k3s, the official Helm chart, CloudNativePG, `cloudflared`, backup CronJob |
| `blueprints/` | Loams configuration as code, applied automatically by the worker |
| `branding/` | Placeholder logo and favicon |

## 0. Facts (checked 2026-10-02)

- **Version pinned: Authentik 2026.8.3**, released 2026-09-17 (GitHub
  `goauthentik/authentik` latest; Helm chart `authentik-2026.8.3` released the
  same day). Image
  `ghcr.io/goauthentik/server:2026.8.3@sha256:ab9b4e8cc4ab3f8d1198d2db6aeea66bafea1963b3f2843589e0d163f97d9849`.
- **No Redis.** Cache, sessions and websockets moved to PostgreSQL in 2025.10.
  The stack is server + worker + PostgreSQL.
- **Open-source edition only.** The blueprints use no model under
  `authentik/enterprise/`; no licence key is ever installed; multi-tenancy
  (`tenants`) is not enabled (OSS doc 38 D447, D458; hosted doc 04 PD37).
- **Cannot run on Cloudflare Workers**: it needs a long-lived Python server,
  a worker and PostgreSQL. Cloudflare is the edge only (Tunnel + DNS + TLS).
- **Minimum size** (Authentik "Docker Compose installation" docs): **2 CPU
  cores and 2 GB RAM** for Authentik and PostgreSQL. The pinned stack also runs
  `cloudflared` and the backup sidecar, and the worker applies blueprints at
  start, so use **2 vCPU / 4 GB RAM / 20 GB disk** for the beta (any
  Hetzner/OVH/DO class with those numbers). Expect roughly 1 to 2 GB resident at idle (not measured on the target VM).
  Keep the dump retention (30 days) in mind for the object store, not the disk.

## 1. Choose the target

- **A. Single VM (beta, start here).** Any Linux VM with Docker Engine 25+ and
  the Compose plugin. No inbound port needs to be open (close 80 and 443; keep
  SSH restricted).
- **B. k3s (later).** When the hosted cloud's k3s cluster exists (doc 04 PD29).
  See section 9.

## 2. Cloudflare Tunnel and DNS for auth.loams.dev

The `loams.dev` zone must be on the Cloudflare account. Pick **one** option.

### Option A: dashboard (recommended, remotely managed)

1. Cloudflare dashboard > **Zero Trust** > **Networks** > **Tunnels** >
   **Create a tunnel** > **Cloudflared** > name it `loams-auth`.
2. On "Install and run", ignore the install command; **copy the token** (the
   long string after `--token`). It goes in `.env` as `TUNNEL_TOKEN`.
3. **Public Hostname** tab > Add:
   - Subdomain `auth`, domain `loams.dev`, path empty
   - Service type **HTTP**, URL **`server:9000`**
     (Compose service name; for k3s use `authentik-server.authentik.svc.cluster.local:80`)
4. Saving creates the proxied DNS record `auth.loams.dev` CNAME
   `<tunnel-id>.cfargotunnel.com` for you. Check it under DNS > Records.

### Option B: `cloudflared` CLI (locally managed)

```sh
cloudflared tunnel login                       # opens a browser, pick loams.dev
cloudflared tunnel create loams-auth           # prints the UUID, writes ~/.cloudflared/<UUID>.json
cloudflared tunnel route dns loams-auth auth.loams.dev
```

Then on the VM, in `deploy/authentik/compose/`:

```sh
mkdir cloudflared
cp cloudflared-config.example.yml cloudflared/config.yml   # put the UUID in it
cp ~/.cloudflared/<UUID>.json cloudflared/credentials.json
chmod 600 cloudflared/credentials.json
# start with the override instead of the plain file:
docker compose -f docker-compose.yml -f docker-compose.cli-tunnel.yml up -d
```

(`TUNNEL_TOKEN` stays empty in this option.)

### Cloudflare settings worth setting

- SSL/TLS mode **Full** (the tunnel is encrypted to the edge; origin is plain
  HTTP inside the Docker network).
- Do **not** put Cloudflare Access in front of `auth.loams.dev`: it would block
  the site's server-side calls to the token and JWKS endpoints.
- Optional WAF rate-limit rule on `/if/flow/` and `/api/v3/flows/` (credential
  stuffing).
- **Once the private network is up (Tailscale by default, Headscale optional),
  take the admin routes off the public name** (section 12): `/if/admin/`, `/if/user/` and the management API are then reachable on
  the tailnet only. Sign-in, OIDC and the flows stay public.

## 3. Object store for backups

Create a private bucket, for example `loams-backups`, and a token that can
read and write only that bucket.

- **Cloudflare R2** (default; hosted doc 04 PD30): R2 > Create bucket; R2 >
  Manage API tokens > Object Read and Write for the bucket. Endpoint
  `https://<account-id>.r2.cloudflarestorage.com`, provider `Cloudflare`.
- **RustFS** (self-hosted, S3 API): endpoint of your RustFS, provider `Other`.

Why rclone: one static binary speaks R2, RustFS and any S3 API from
environment variables, and `rclone delete --min-age` gives retention without a
repository to initialise. restic would add deduplicated, encrypted snapshots;
for a database of tens of megabytes a plain custom-format `pg_dump` is easier to
restore by hand. Add object-store lifecycle rules or bucket versioning if you
want a second line of defence.

## 4. Generate secrets and start (target A)

On the VM:

```sh
git clone https://github.com/ostrium-labs/loams.git
cd loams/deploy/authentik/compose
cp .env.example .env && chmod 600 .env
```

Fill `.env`:

| Variable | How |
|---|---|
| `AUTHENTIK_SECRET_KEY` | `openssl rand -base64 60 \| tr -d '\n'` |
| `PG_PASS` | `openssl rand -base64 36 \| tr -d '\n'` (no `#`, quotes or `$`) |
| `AUTHENTIK_BOOTSTRAP_PASSWORD` / `_EMAIL` | the `akadmin` password and your email |
| `SMTP_*`, `SMTP_FROM` | your relay (Resend, SES, Postmark, ...). Verification and recovery mail needs it |
| `LOAMS_CLOUD_CLIENT_SECRET` | `openssl rand -hex 32` (this becomes `AUTH_AUTHENTIK_SECRET`) |
| `HEADSCALE_OIDC_CLIENT_SECRET` | `openssl rand -hex 32` (the secret of the `headscale` client; only used if you switch the tailnet to Headscale, [`headscale.md`](headscale.md)) |
| `TAILNET_PROVIDER`, `TAILNET_LOGIN_SERVER`, `TS_AUTHKEY`, `TS_TAGS`, `TS_HOSTNAME` | optional, only with `--profile tailnet`: [`tailnet.md`](tailnet.md). Defaults: `tailscale`, empty, empty, `tag:authentik`, `ak-1` |
| `LOAMS_PREVIEW_ORIGIN` | exact preview origin of the Worker, e.g. `https://loams-cloud-preview.<you>.workers.dev` |
| `LOAMS_ENROLLMENT_OPEN` | `false` (invitation-only) for the beta |
| `TUNNEL_TOKEN` | from section 2, option A |
| `BACKUP_S3_*` | from section 3 |

Store `.env` and the secret key in the password manager: the database backup
is useless without `AUTHENTIK_SECRET_KEY`.

```sh
docker compose config -q          # validates, fails on any missing variable
docker compose up -d --build      # builds the small backup image locally
docker compose ps                 # wait until server, worker, postgresql are healthy
docker compose logs -f worker     # blueprint lines: "Blueprint applied" / "Loams - ..."
```

## 5. First admin login and blueprint check

1. Open **https://auth.loams.dev/if/admin/** and sign in as `akadmin` with
   `AUTHENTIK_BOOTSTRAP_PASSWORD`. (Do not use `/if/flow/initial-setup/`; the
   bootstrap variables replace it.) Then remove the two `AUTHENTIK_BOOTSTRAP_*`
   lines from `.env`.
2. Enrol MFA for `akadmin` (top right > Settings > MFA devices); it is in the
   built-in `authentik Admins` group, so the Loams flow demands it.
3. **Customization > Blueprints**: `Loams - Groups`, `Loams - Flows`,
   `Loams - OIDC clients`, `Loams - Brand` must show status **Successful**.
   If one shows "Error", open it and read the message; fix the YAML or `.env`
   and run `docker compose restart worker`.
4. **Applications > Applications**: `Loams Cloud`, `Loams Desktop`,
   `Loams Mobile`. **Directory > Groups**: `loams-admins`, `loams-beta`.
5. Health: `curl -fsS https://auth.loams.dev/-/health/ready/` returns 200.
6. Discovery: `curl -fsS https://auth.loams.dev/application/o/loams-cloud/.well-known/openid-configuration | jq .issuer`
   prints `"https://auth.loams.dev/application/o/loams-cloud/"`.
7. Put yourself in `loams-admins`: create your own user (Directory > Users >
   Create), add it to `loams-admins`, enrol MFA when prompted.

### Beta access: invitations

With `LOAMS_ENROLLMENT_OPEN=false` the sign-up page requires an invitation:
**Directory > Invitations > Create** (flow `Loams sign up`, optionally
single-use, with an expiry and a pre-filled email), then send the generated
link. New users get an inactive account, verify their email, and are added to
`loams-beta`. For open sign-up set `LOAMS_ENROLLMENT_OPEN=true` in `.env`, run
`docker compose up -d worker`, then re-apply the blueprints (blueprints read
`.env` values only when they are applied, and a restart alone does not apply
unchanged files):

```sh
docker compose exec worker sh -c 'for f in /blueprints/loams/*.yaml; do ak apply_blueprint "$f"; done'
```

The same command is needed after changing `LOAMS_CLOUD_CLIENT_SECRET`,
`LOAMS_PREVIEW_ORIGIN` or `LOAMS_AUTH_DOMAIN`. (Authentik also re-applies every
blueprint on a schedule, so a forgotten step should self-heal within the hour.)

## 6. Values for the site (Workers secrets)

The OIDC issuer is **per application**, not the bare host. Auth.js discovers
`<issuer>/.well-known/openid-configuration`, which Authentik serves under the
application slug:

| Secret | Value |
|---|---|
| `AUTH_AUTHENTIK_ISSUER` | **`https://auth.loams.dev/application/o/loams-cloud/`** (keep the trailing slash) |
| `AUTH_AUTHENTIK_ID` | `loams-cloud` (fixed by the blueprint) |
| `AUTH_AUTHENTIK_SECRET` | the value of `LOAMS_CLOUD_CLIENT_SECRET` in `.env` |

```sh
cd loam-cloud
printf 'https://auth.loams.dev/application/o/loams-cloud/' | wrangler secret put AUTH_AUTHENTIK_ISSUER
printf 'loams-cloud'                                        | wrangler secret put AUTH_AUTHENTIK_ID
# paste LOAMS_CLOUD_CLIENT_SECRET from deploy/authentik/compose/.env on the VM when prompted
wrangler secret put AUTH_AUTHENTIK_SECRET
```

(`AUTH_SECRET` for Auth.js is separate: `openssl rand -base64 33`.)

Registered redirect URIs for `loams-cloud`:
`https://loams.dev/api/auth/callback/authentik` and
`${LOAMS_PREVIEW_ORIGIN}/api/auth/callback/authentik`. Scopes: `openid email
profile` already carry the `groups` claim (only groups starting with `loams-`:
`loams-admins`, `loams-beta`); `groups` and `offline_access` can be requested
explicitly. Tokens are signed RS256 with the instance's self-signed key, which
Auth.js fetches from the JWKS. If you prefer, you can read the id and secret in
the UI (Applications > Providers > loams-cloud), but the env value is the
source of truth.

Desktop and mobile clients (public, PKCE S256, no secret), same issuer
pattern:

| Client | Issuer | Client id | Redirect |
|---|---|---|---|
| desktop | `https://auth.loams.dev/application/o/loams-desktop/` | `loams-desktop` | `http://127.0.0.1:<any port>/callback` (also `[::1]`) |
| mobile | `https://auth.loams.dev/application/o/loams-mobile/` | `loams-mobile` | `https://loams.dev/app/auth/callback` or `dev.loams.app:/oauth2redirect` |

The RFC 8628 device flow is enabled through the `loams-device-code` flow on the
brand (OSS doc 37 section 7.2, option 3).

## 7. Backups and restore drills

The `backup` service runs `pg_dump --format=custom` nightly at `BACKUP_AT`
(03:15 UTC), checks the dump with `pg_restore --list`, uploads it to
`s3://$BACKUP_S3_BUCKET/$BACKUP_PREFIX/authentik-<UTC timestamp>.dump`, and
deletes objects older than `BACKUP_RETENTION_DAYS` (30). Set
`BACKUP_HEARTBEAT_URL` to a dead-man's-switch ping to be told when it stops.

```sh
docker compose exec backup backup.sh now             # one backup right now
docker compose exec backup restore.sh list           # what is in the bucket
```

**Monthly drill (safe, touches nothing live):** restores the newest dump into
a scratch database, counts users and applications, drops it.

```sh
docker compose exec backup restore.sh verify latest   # prints "DRILL OK"
```

**Real restore** (database lost or corrupted):

```sh
docker compose stop server worker
docker compose exec backup restore.sh apply latest    # or an object name from `list`
docker compose start server worker
```

For a new VM: install Docker, restore the same `.env` (same
`AUTHENTIK_SECRET_KEY`!), `docker compose up -d postgresql backup`, run
`restore.sh apply latest`, then `docker compose up -d`. The tunnel token can be
reused, but stop the old host first. The blueprints, branding and compose files
come from Git; media uploaded in the UI (`data` volume) is not backed up
(nothing in the Loams setup depends on it).

## 8. Upgrades

1. Read the release notes at https://docs.goauthentik.io/releases for the
   target version. Authentik releases are monthly; **upgrade one minor version
   at a time, never skip** (2026.8 > 2026.11 ...); patch releases (2026.8.x) can
   be taken directly.
2. `docker compose exec backup backup.sh now` and confirm the object exists.
3. Edit the `x-authentik-image` anchor in `docker-compose.yml` to the new tag
   **and its digest** (`docker buildx imagetools inspect ghcr.io/goauthentik/server:<tag>`
   prints it), then `docker compose pull && docker compose up -d`. Migrations
   run at start; watch `docker compose logs -f server worker`.
4. Re-check Customization > Blueprints are all Successful, the health
   endpoint, and a test sign-in from the site.
5. Rollback: `docker compose down`, revert the image line, `restore.sh apply`
   the pre-upgrade dump (Authentik does not support downgrading a migrated
   database).
6. Postgres major upgrades are a dump and restore (the same scripts), done
   separately from Authentik upgrades.

Before each Authentik bump also confirm nothing the blueprints use moved into
`authentik/enterprise/` (OSS doc 38 D458).

## 9. Target B: k3s with the Helm chart

```sh
cd deploy/authentik
# 1. secrets (see helm/secrets.example.yaml for the exact commands)
kubectl create namespace authentik   # or let kustomize create it
# 2. namespace, CloudNativePG cluster, backup CronJob, cloudflared, blueprint + branding ConfigMaps
kubectl apply -k .
# 3. the official chart (GPL-3.0, referenced, not vendored)
helm repo add authentik https://charts.goauthentik.io
helm upgrade --install authentik authentik/authentik --version 2026.8.3 \
  -n authentik -f helm/values.yaml
```

Prerequisites: the CloudNativePG operator (`kubectl apply --server-side -f`
the release manifest from cloudnative-pg.io). In the tunnel's Public Hostname
use `authentik-server.authentik.svc.cluster.local:80`. Blueprints reach the
worker as the ConfigMap `loams-blueprints` (mounted by `blueprints.configMaps`);
edit a file, `kubectl apply -k .`, and the worker re-applies it when the mounted file changes (kubelet sync, about a minute). After changing a Secret value, `kubectl -n authentik rollout restart deploy/authentik-worker` and re-apply with `kubectl -n authentik exec deploy/authentik-worker -- sh -c 'for f in /blueprints/mounted/cm-loams-blueprints/*.yaml; do ak apply_blueprint "$f"; done'`.
Backups run as the `authentik-backup` CronJob with the same retention.
Restore drill on k3s: `kubectl -n authentik create job --from=cronjob/authentik-backup manual-1`
to take a dump; to restore, scale the `authentik-server`/`authentik-worker`
Deployments to 0, then `pg_restore --clean --if-exists --no-owner -d authentik`
from a pod that has the dump (an `rclone copyto` followed by `pg_restore`).

## 10. Operational notes

- Logs: `docker compose logs --tail 200 server worker`.
- Authentik analytics, update checks and error reporting are disabled.
- Only `127.0.0.1:9000` is published on the host; nothing is reachable except
  through Cloudflare. Remove that port mapping if you do not need local checks.
- Rotating `LOAMS_CLOUD_CLIENT_SECRET`: change it in `.env`, `docker compose up
  -d worker`, re-apply the blueprints (section 5), then update
  `AUTH_AUTHENTIK_SECRET` in Workers.
- Cost: the stack is free software; the VM, R2 storage (a few MB per dump) and
  an SMTP relay are the only costs.

## 11. What was verified, and what was not

Verified locally on 2026-10-02 (Podman 6.1.1 with `docker-compose` 5.2.0, the
pinned images): `docker compose config`; `postgresql`, `server`, `worker` and
`backup` healthy; `/-/health/ready/` returned 200; the four Loams blueprints
applied "Successful"; OIDC discovery and RS256 signing; a headless
authorization-code sign-in for `loams-cloud` returned an ID token with the
`loams-` groups only; an admin was forced into TOTP/WebAuthn enrolment; the
invitation-only toggle in both positions; loopback, https and custom-scheme
redirect URIs accepted and others refused; the enterprise licence summary
reported `unlicensed`; backup, `restore.sh verify` and `restore.sh apply`
against an S3 endpoint (rclone's built-in S3 server); `kubectl kustomize` and
`helm template` for target B.

Not verified: `cloudflared` against a real tunnel (needs your token), real SMTP
delivery of verification and recovery mail, Cloudflare R2 itself, the k3s path
on a live cluster, and the site's Auth.js integration.

## 12. Admin routes on the tailnet only (after the tailnet is up)

Authentik's admin interface and management API are the most valuable thing on
the public name, and nobody but an operator needs them. When the private
network is running ([`tailnet.md`](tailnet.md): hosted Tailscale on your own
account by default, Headscale as the alternative, selected by configuration
only), move them off the internet. The identity flows people use stay public.
`auth.loams.dev` itself stays on Cloudflare Tunnel.

| Reachable on the public name `auth.loams.dev` | Tailnet only (`tag:authentik`, port 9000, `group:ops`) |
|---|---|
| `/if/flow/*` (sign-in, enrolment, recovery, device code), `/application/o/*` (OIDC, JWKS), `/api/v3/flows/*`, `/static/*`, `/media/*`, `/-/health/*` | `/if/admin/*`, `/if/user/*`, the management API (`/api/v3/{core/users,core/groups,core/tokens,providers,sources,stages,policies,outposts,...}`) |

Steps (the provider-specific setup is in [`tailnet.md`](tailnet.md); the full
verification checklist is in [`headscale.md`](headscale.md), section 8):

1. Set the tailnet variables in `.env` (`TAILNET_PROVIDER`, `TAILNET_LOGIN_SERVER`,
   `TS_AUTHKEY` with a key tagged `tag:authentik`, `TS_TAGS`, `TS_HOSTNAME`) and run
   `../../tailnet/check-env.sh .env`.
2. Start the node: `docker compose --profile tailnet up -d`. The `tailscale`
   service shares the server's network namespace (userspace networking), so
   `http://ak-1:9000/if/admin/` works from an operator device on the tailnet.
   (A tailscale client installed on the host works too, with the override
   `docker compose -f docker-compose.yml -f ../../tailnet/authentik-tailnet/docker-compose.tailnet.yml up -d`
   and `TAILNET_IP` in `.env`.) On k3s: `kubectl apply -k deploy/tailnet/tailscale`.
3. Add the path rule of
   [`deploy/tailnet/authentik-tailnet/cloudflared-admin-deny.example.yml`](../../../deploy/tailnet/authentik-tailnet/cloudflared-admin-deny.example.yml)
   to the tunnel (dashboard: a published application route for `auth.loams.dev`
   with that path and the service **HTTP Status 404**, ordered above the
   catch-all).
4. From a device that is **not** on the tailnet, confirm those paths return 404
   and that sign-in to `loams.dev`, invitation enrolment, recovery and the
   device-code flow still work. If a flow breaks, remove the rule and report
   the path.

If the tailnet is down, use the break-glass path in `headscale.md` (a recovery
key through SSH from the provider's console). Do not remove the path rule.
The blueprint `loams-05-headscale.yaml` adds the `headscale` OAuth2 client and
the groups `loams-net-users` and `loams-net-ops`; `HEADSCALE_OIDC_CLIENT_SECRET`
is required in `.env` (any random value unless the tailnet is Headscale).

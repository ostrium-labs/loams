# Private network (tailnet): Tailscale by default, Headscale as the alternative

> **Imported on 2026-10-19.** Written in the former private `ostrium-labs/loams-platform` repository; open source (Apache-2.0) in the monorepo since the owner's order of 2026-10-19. "This repository", "proprietary" and "private" below describe that repository and the open-core boundary of the time, which no longer holds; see [the platform docs index](../README.md).

Owner ruling 2026-10-02: "for now make it **BYOK Tailscale**, make it configurable
so I can add Headscale in the future." The private network of the hosted Loams
service (SSH and admin access with no public ports, k3s, TiKV and WeSQL across
sites, backups over private paths, the `loam-bench` runner, and Zitadel's admin
console once IA1 deploys it) is therefore a **pluggable tailnet provider**:

| `TAILNET_PROVIDER` | Control server | `TAILNET_LOGIN_SERVER` | Who runs it |
|---|---|---|---|
| `tailscale` (default) | Tailscale's hosted service, on **your own** account (bring your own key) | empty | Tailscale Inc. |
| `headscale` (later) | Headscale at `https://headscale.loams.dev` | that URL | you ([`headscale.md`](headscale.md)) |

Switching is a **configuration change only**: the clients, the tags and the policy
file are the same. The design is OSS
`docs/design/43-private-networking.md` (now provider-neutral); the public
`auth.loams.dev` stays on Cloudflare Tunnel in both cases.

Layout of [`deploy/tailnet/`](../../../deploy/tailnet):

| Path | What |
|---|---|
| `policy/policy.hujson` | The default-deny policy, **one file for both providers**; `policy/lint.py` is its offline check |
| `tailscale/` | k3s manifests for a Tailscale node (`kustomization.yaml`, `k8s/secrets.example.yaml`). The auth host's node was removed with the old identity provider (D1600); Zitadel's comes with IA1 ([`zitadel.md`](zitadel.md)) |
| `headscale/` | The alternative: compose, k3s, config, backup, checker ([`headscale.md`](headscale.md)) |
| `check-env.sh` | Checks `TAILNET_PROVIDER` and `TAILNET_LOGIN_SERVER` agree |
| `../../.github/workflows/platform.yml` | Lint and Headscale check (jobs `tailnet-lint`, `tailnet-headscale`); Tailscale test and apply run from a separate private deployment repository (step 3) |

## 0. Facts (checked 2026-10-02)

- **Control server selection.** `tailscale up --login-server=<url>` selects the
  control server; unset means Tailscale's hosted service. The
  `tailscale/tailscale` container has **no `TS_LOGIN_SERVER` variable** (read
  from `cmd/containerboot` at v1.102.5): pass it through `TS_EXTRA_ARGS`
  (`--login-server=...`). A compose `tailscale` service must do exactly that, and
  only when `TAILNET_LOGIN_SERVER` is non-empty (the removed auth-host service
  did; the `auth.loams.dev` bundle's replacement comes with IA1).
- **Versions and licences.** Tailscale client v1.102.5 (2026-09-29, BSD-3-Clause;
  the hosted control server is proprietary and not open source). Headscale v0.29.4
  (2026-09-23, BSD-3-Clause; minimum client v1.80). `tailscale/gitops-acl-action`
  v1.5.2 (2026-04-27, BSD-3-Clause). `tailscale/github-action` v4.2.0 (2026-09-22,
  BSD-3-Clause).
- **Credentials.** (a) **Auth key**: tagged, reusable or single-use, created in
  the admin console; used once, at registration. (b) **OAuth client** ("trust
  credential"): a client id and secret with scopes. `gitops-acl-action` needs
  `policy_file`; `tailscale/github-action` and containerboot (`TS_CLIENT_ID` and
  `TS_CLIENT_SECRET`) mint **ephemeral, tagged, pre-approved** keys from a client
  with the writable `auth_keys` scope, and the node must be given tags
  (`--advertise-tags`). API keys expire in 90 days; OAuth clients do not.
  Workload-identity federation (GitHub OIDC, no stored secret) also exists; the
  workflow here uses the OAuth client the owner specified.
- **Tailscale Personal (free) plan**, from tailscale.com/pricing and
  /pricing/faq: **up to 6 users, unlimited user devices, up to 50 tagged
  resources to start, up to 3 ACL groups, 1,000 ephemeral-resource minutes per
  month**, basic Tailscale SSH (up to 5 hosts), MagicDNS, subnet routers, exit
  nodes, Serve and Funnel, "nearly all of Tailscale's features". Paid-only (not on
  Personal): SCIM, device-posture integrations, MDM configuration, advanced roles
  (Standard, $8 per user per month); just-in-time access, advanced SSH, network
  flow logs, log streaming (Premium, $18). Two things matter for Loams:
  - The FAQ describes Personal as **for non-commercial use** (homelab, home VPN).
    A hosted commercial service is outside that. **Decision for the owner**: run
    on Personal only for non-commercial testing and move to a paid plan before
    the hosted service has users, or start on a paid plan.
  - Tagged resources are capped at 50 and ACL groups at 3 (this policy uses one
    group and about eleven tags).
- **Custom OIDC (your own IdP, later Zitadel) on Free.** Tailscale's GA announcement
  (tailscale.com/blog/custom-oidc-ga): "Any custom OIDC provider can be used for up
  to three users on our Free plan"; free or self-hosted providers are listed under
  Starter, advanced or enterprise providers under Premium and Enterprise. The
  current Personal limit is six users (pricing page), and the current custom-OIDC
  documentation does not restate the plan matrix, so **confirm the cap in the
  sign-up flow**. Requirements (docs, 2026-10-02): the email domain must be a
  custom domain that serves a **WebFinger** endpoint at
  `https://<domain>/.well-known/webfinger` pointing at the OIDC issuer; the IdP
  needs the `openid`, `profile`, `email` scopes and ES256 or RSA >= 2048-bit
  signatures; all users of the domain use the same IdP; **no user or group
  provisioning**; and it is chosen **when the tailnet is created** (an existing
  account can migrate only if it uses a custom domain, never `@gmail.com`).
  Zitadel was not checked against the documented provider list; it is a
  standards-compliant OIDC provider: treat it as **likely, not verified**
  until a sign-up is tried. For now the simple path is a single owner identity
  (GitHub, Google or Microsoft sign-in) on a tailnet whose `group:ops` lists that
  email; Zitadel OIDC for the tailnet is a later switch (after IA1 deploys it), and also needs a
  WebFinger route on `loams.dev` (a Cloudflare Worker rule).
- **Policy as code.** Both providers read HuJSON. Differences that matter:

  | | Tailscale | Headscale v0.29 |
  |---|---|---|
  | Apply | API (`gitops-acl-action apply`); the admin console also edits it, so CI is the only writer by convention | file on the server, reload with `SIGHUP` |
  | Test | `gitops-acl-action test` calls the API validator; `tests` are evaluated by the API without needing nodes (Tailscale docs; not exercised here) | `headscale policy check`; `tests` need a **node for every tag/user named** (hence `check.sh` seeds a scratch database) |
  | Users | identity-provider email | `name@` or the OIDC user (name or email) |
  | Groups | `group:` lists, up to 3 on Personal; no IdP-group sync without SCIM (paid) | `group:` lists; OIDC groups never usable in policy |
  | Autogroups | all (`autogroup:member`, `tagged`, `self`, `internet`, `admin`, `owner`, `nonroot`, ...) | `internet`, `member`, `tagged`, `self`, `nonroot`, `danger-all` only |
  | `ssh` | Tailscale SSH (Personal: basic, 5 hosts) | supported; this policy uses OpenSSH over `tcp:22` grants instead, which works on both |
  | `nodeAttrs` | full (Funnel, Taildrop, ...) | supported subset; no Serve or Funnel |
  | `postures`, `srcPosture`, `ipsets` | supported (posture is paid) | **not supported** |
  | `tagOwners` | required for every tag | required; **a tagged pre-auth key fixes the tags and a client-requested `--advertise-tags` is refused** (checked 2026-10-02) |

  `policy.hujson` therefore uses only `groups`, `tagOwners`, `grants` and `tests`;
  `policy/lint.py` and the Headscale check fail the build if a Tailscale-only
  section sneaks in.

## 1. Choose the provider (the knob)

```sh
# the node's .env (and the Secret `tailnet-auth` on k3s)
TAILNET_PROVIDER=tailscale          # default
TAILNET_LOGIN_SERVER=               # empty = Tailscale's hosted service
TS_AUTHKEY=tskey-auth-...           # tagged key from your tailnet (never committed)
TS_TAGS=tag:zitadel                 # the auth host; one tag per role
```

To move to Headscale, deploy it ([`headscale.md`](headscale.md)), mint a tagged
pre-auth key there, and change **only**:

```sh
TAILNET_PROVIDER=headscale
TAILNET_LOGIN_SERVER=https://headscale.loams.dev
TS_AUTHKEY=<headscale preauth key, --tags tag:zitadel>
```

then set the repository variable `TAILNET_PROVIDER=headscale` (the CI job that
applies the policy to Tailscale stops; the policy is reloaded on the Headscale
host) and recreate the node (delete its state volume first if it held a node of
the other provider).
`deploy/tailnet/check-env.sh .env` fails if the two variables disagree. For
Headscale the compose service drops `--advertise-tags`; the tags come from the key.
On k3s the same switch is the Secret's `TS_EXTRA_ARGS` ([`tailscale/k8s/secrets.example.yaml`](../../../deploy/tailnet/tailscale/k8s/secrets.example.yaml)).

## 2. Create the tailnet and the credentials (owner, Tailscale)

UI paths below are from Tailscale's documentation of 2026-10-02 and were **not
clicked through here**; names may differ slightly.

1. **Create the tailnet.** Sign up at login.tailscale.com with the identity you
   will use as operator. Simple path now: GitHub, Google or Microsoft sign-in
   (the Personal plan). Later, to use Zitadel: sign up with an `@loams.dev`
   address and choose "Sign in with OIDC" (needs the WebFinger route and a
   Zitadel application for Tailscale; see section 0).
2. **Put the policy in first.** Admin console, **Access controls**: paste
   `deploy/tailnet/policy/policy.hujson` after replacing `admin@loams.dev` (in
   `group:ops` and in the tests) with your sign-in email. Tags must exist in
   `tagOwners` before keys can carry them.
3. **OAuth client for CI** (Settings, **Trust credentials** / OAuth clients):
   scope `policy_file` (write). Copy the client id and secret once. The
   monorepo's CI (`.github/workflows/platform.yml`) only lints the policy and
   runs Headscale's checker; it never tests or applies it through the Tailscale
   API (owner ruling 2026-10-19). The apply job lives in a **separate private
   deployment repository**: it holds the `TS_OAUTH_CLIENT_ID` and
   `TS_OAUTH_SECRET` secrets, pins the policy from this repository, and runs
   `tailscale/gitops-acl-action` (`test` on pull requests, `apply` on its main
   branch). Optional variable `TS_TAILNET` with your tailnet name (default
   `-`, the client's own). Without that repository, paste the policy in the
   admin console.
4. **Auth key for a server node** (Settings, **Keys**, Generate auth key): its
   role tag (for the auth host, `tag:zitadel`), **reusable off** (single use) with a short expiry, or
   reusable if the container's state volume may be lost; pre-approved on if the
   tailnet requires device approval. Put it in `.env` as `TS_AUTHKEY`. (Alternative
   with no key to rotate: a second OAuth client with the writable `auth_keys`
   scope and the same tag, used as `TS_CLIENT_ID` and `TS_CLIENT_SECRET`;
   it needs `--advertise-tags` and mints an ephemeral key per start.)
5. **Join your PC.** Install the Tailscale client, `tailscale up`, sign in. It is
   an untagged device owned by you, so it gets what `group:ops` is granted.
6. **Join the auth host.** Planned: the `auth.loams.dev` bundle (IA1,
   [`zitadel.md`](zitadel.md)) brings the node, tagged `tag:zitadel`. Check its
   `.env` with `deploy/tailnet/check-env.sh <path>/.env` before starting it.
7. **Join phones.** Install the Tailscale app (iOS, Android), sign in with the
   same identity. No custom server setting is needed for Tailscale (it is for
   Headscale: `headscale.md`, section 6).
8. **Check.** From the PC, `tailscale status` lists the nodes, and a port the
   policy grants `group:ops` on a tagged node (MagicDNS name, or the node's `100.x`
   address) answers; from a device not on the tailnet it does not.

Server nodes (`tag:k3s`, `tag:tikv`, ...): same pattern, one tagged key per role,
`tailscale up --authkey ... --hostname ...` (add `--login-server` only for
Headscale). The k3s flags in `headscale.md` section 6 apply unchanged.

## 3. Policy changes

Edit `deploy/tailnet/policy/policy.hujson` in a pull request. CI runs
`lint.py`, the Headscale checker (so the file stays portable), and, for
`TAILNET_PROVIDER=tailscale` with the secrets set, `gitops-acl-action test`. On
merge to `main` the same action **applies** it. The Tailscale `test` and `apply`
steps could not be run here (they need a tailnet and an OAuth client; the action
is a GitHub composite action around Tailscale's `gitops-pusher`, so there is no
local mode): the first merge after the secrets are set is their first real run.
Locally, `python3 deploy/tailnet/policy/lint.py` and
`deploy/tailnet/headscale/policy/check.sh` (`DOCKER=podman` works) cover syntax,
the shared subset and the access tests under Headscale's evaluator.

## 4. Zitadel's admin console on the tailnet

Planned (IA1). Zitadel's admin console and management APIs are never public
(D1617, carrying D596's rule over); the node that publishes them on the tailnet as
`tag:zitadel` comes with the `auth.loams.dev` bundle. See `headscale.md`
section 8 and [`zitadel.md`](zitadel.md). Use the policy grant, not an IP check,
as the control.

## 5. What was verified, and what was not

Verified 2026-10-02 (Podman 6.1.1, `docker-compose` 5.2.0): `docker compose config`
for the auth host's compose stack of the time (removed by D1600) **with** and
**without** the `tailnet` profile; the
`TS_EXTRA_ARGS` selection for all four combinations; `check-env.sh`;
`policy/lint.py`; Headscale v0.29.4 `policy check` with the shared policy through
the container (`headscale/policy/check.sh`) and `check-config.sh`;
`kubectl kustomize` for `tailscale/` and for `headscale/` with
`--load-restrictor=LoadRestrictionsNone`; and an end-to-end run with real
containers: a Headscale v0.29.4 carrying this policy, the `tailscale` service's
entrypoint script and image (v1.102.5) joined as the auth host's tag (now
`tag:zitadel`) from a tagged pre-auth key, and an operator device reached its
admin port over the tailnet with other ports dropped.

**Not verified:** anything against Tailscale's hosted service (no account here):
sign-up, custom OIDC with your own IdP, the OAuth client scopes, `gitops-acl-action`
`test`/`apply` (needs the secrets), `TS_AUTHKEY` registration, and the free-plan
limits are from Tailscale's published pages, not from a live account. The k3s
manifest was rendered, not applied to a cluster. The `-` default for `TS_TAILNET`
is Tailscale's API convention for "the client's own tailnet"; confirm on the first
run.

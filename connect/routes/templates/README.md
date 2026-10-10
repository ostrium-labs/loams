![Loams — Your data. Your bucket.](../../../docs/assets/loams-banner.svg)

# `connect/routes/templates/` — the import routes

CN1 Task 12's home for route templates, and the route half of CN1 Task 15 (D628).
These are the paths that carry data **into** Loams’ own collaboration
applications. Nothing here executes yet: Task 12's `loams-connect` Java service is
deferred, and these templates are the declarative half of it, written so the mappings
are reviewable and diffable before any Java exists.

## What is here

| Template | Route | Idempotence carried by |
|---|---|---|
| `github-issues-to-forgejo` | GitHub issues → Forgejo issues | Forgejo's own issue key, via `Idempotency-Key` |
| `github-issues-to-itsplane` | GitHub issues → ItsPlane issues | ItsPlane's human identifier (`MKT-42`) |
| `slack-messages-to-zulip` | Slack messages → Zulip messages, topic-per-thread | Zulip's `Idempotency-Key` on the message post |

Jira → ItsPlane is specified by the same mapping as GitHub → ItsPlane: both are
issue trackers with a title, a body and a state, and the projection differs only in
field names. It lands with the P2 wave rather than as a fourth file, because the
second copy would be a near-duplicate of `itsplane-import.yaml.tmpl`.

## Why the three targets are connectors at all

Without a sink there is nowhere for an import to land, so CN1 Task 15 adds Zulip,
ItsPlane and Forgejo to the registry as **P1 but unstarred** native connectors. They
are unstarred because D628 keeps D358's précis' 21 unchanged: these three are Loams’
own applications reached over HTTP, not part of the hot-path set. Camel 4.22.1 has no
component for any of them (`camel-zulip`, `camel-plane` and `camel-gitea` are all
absent at that tag), so D354's "buy first" has nothing to buy and all three are
hand-written Rust in `loams-flow`.

## What is not here, and why

- **No Java, and no custom Camel component.** Task 12 defers the Java service; these
  routes use only Camel's YAML DSL, its built-in `jq` and `jsonata` languages, and
  `camel-cloudevents`.
- **No runnable source.** Slack, GitHub and Jira are Appendix A's P2 rows on Camel's
  `slack`, `github2` and `jira` components, all three present at tag `camel-4.22.1`.
  They land in **CN2**, so these routes document the contract the P2 wave must satisfy
  rather than running today.
- **Nothing from `ostrium-labs/loams-plugins`.** That repository's Zulip, ItsPlane and
  Forgejo adapters are read-only by policy, stated in four places, with tests
  asserting every request is a `GET`. Its Forgejo adapter's own header gives the
  reason: Forgejo derives a token's scope from the HTTP method, so a `GET`-only
  adapter can never hold a write scope. The write credentials therefore live in
  Loams’ `SecretStore` (D189) and these writes happen from the Rust side. CN1
  "Rulings made during execution" row 13 records that decision.

## Three constraints every template here is shaped by

1. **Projection first.** A GitHub issue or a Slack message is mostly fields no Zulip
   message or ItsPlane issue needs. D356's warehouse rule applies to an import too:
   every route sets an explicit body rather than forwarding the payload whole.
2. **Rate limits are upstream facts, and two of the three are hostile.**
   - Zulip: 200 requests/60 s, **shared across every endpoint with no read-only
     tier**, so an import competes with dashboard reads for one budget. It publishes
     `X-RateLimit-Limit`/`-Remaining`/`-Reset`, so the sink self-throttles from those
     headers rather than waiting for a 429.
   - ItsPlane: 100 requests/s per API key, but **no remaining-budget header**, so the
     caller must budget rather than read. Its issue listing also truncates silently
     at `limit` with no total and no cursor, so these routes page by their own filter
     window and never treat a returned count as complete.
   - Forgejo: **no upstream rate limit at all**, so the only brake is this route's own
     in-flight concurrency. Its `limit` is silently clamped to 50, so the templates
     send 50 rather than discovering the clamp.
3. **Two tokens for Forgejo.** Forgejo fixes a token's scopes at mint time and derives
   the required scope from the HTTP method, so `read:*` and `write:issue` cannot
   coexist. `forgejo-import.yaml.tmpl` checks for the write token by name before the
   first write, so a missing provisioning step fails with the credential's name rather
   than a 403.

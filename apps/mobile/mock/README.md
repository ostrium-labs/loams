![Loams — Your data. Your bucket.](../../../docs/assets/loams-banner.svg)

# loams-mock

A small, stateful Connect server for the app protos (`loams.instance.v1`, `devices`, `approvals`, `operations`, `notifications`), so both apps can be built and tried without a Loams server. It also serves the HTTP endpoints the apps need around the RPCs: the OAuth token endpoint (pairing grant, token exchange, refresh), the instance JWKS, and a fake Authentik for browser sign-in.

**Everything here is test-only.** Tokens are fixed fake strings (`mock-access-…`), the instance key is derived from a public label, and the server binds loopback only.

## Why Go

The lightest option that is also the most faithful one:

- **connect-go is the reference Connect implementation.** It serves Connect, gRPC and gRPC-Web on one port, so the apps' connect-kotlin and connect-swift clients are tested against the protocol as specified, not against a hand-rolled server. (connect-kotlin is client-only, so a Kotlin/JVM mock would have meant writing the server side of the protocol by hand.)
- **It builds in seconds to one static binary** with no runtime, on Linux, Windows and macOS. CI attaches prebuilt binaries, so running the Android app does not even need Go installed.
- **It stays small**: a few hundred lines, standard library HTTP, three dependencies (`connectrpc.com/connect`, `cloudflare/circl` for HPKE, `qrterminal`).
- A Rust mock (connect-rust) would match the server's stack, but it is a much heavier build, and the main repository's AP0 already plans `loams-apps-mock` in Rust. When that ships with its scenario files, the conformance suites can run against it as well; this one stays the quick local loop.

## Run

```sh
cd mock
go run ./cmd/loams-mock            # listens on 127.0.0.1:8084
```

It prints a pairing payload (valid 5 minutes) and a QR code. The Android emulator reaches the host's loopback as `10.0.2.2`, so the default `-public-url http://10.0.2.2:8084` is right for the emulator. For a USB device, run `adb reverse tcp:8084 tcp:8084` and use `-public-url http://127.0.0.1:8084`.

| Flag | Default | |
|---|---|---|
| `-listen` | `127.0.0.1:8084` | loopback only; anything else is refused |
| `-public-url` | `http://10.0.2.2:8084` | the issuer the printed QR names |
| `-heartbeat` | `15s` | Watch heartbeat interval |
| `-tick` | `2s` | running operations advance by 5 % per tick |
| `-approval-every` | `0` | create a demo approval periodically |
| `-seed` | `true` | demo approvals, operations and notifications |

## Test controls (not part of any Loams API)

| Request | Effect |
|---|---|
| `GET /mock/pairing` | a fresh pairing payload for `usr_alice` |
| `POST /mock/approvals` | a new pending approval, its operation, an inbox entry, and a sealed push to every registered push target |
| `POST /mock/drop-streams` | ends every Watch stream with `unavailable`, to exercise resume |
| `POST /mock/tick` | advance running operations once |
| `GET /mock/push-log?token=…&after=N` | what a push gateway would have received: routing fields and the sealed payload only |

Try it with curl (Connect's JSON codec):

```sh
curl -s -X POST -H 'Content-Type: application/json' -d '{}' \
  http://127.0.0.1:8084/loams.instance.v1.InstanceService/GetInstance
curl -s -X POST -H 'Content-Type: application/json' -H 'Authorization: Bearer mock-access-usr_alice' -d '{}' \
  http://127.0.0.1:8084/loams.approvals.v1.ApprovalService/ListApprovals
```

## What it checks for real

- Pairing codes are single use, expire after 5 minutes, accept exactly one of `code` or `user_code`, and a pairing burns after 5 wrong user codes.
- Decision proofs are verified: ES256 over the canonical `DecisionClaims` with the decision key registered at pairing, matching id, revision and decision, `iat` within 5 minutes, `jti` single use. The requester (or the user an agent acts for) cannot approve; a rejection needs a reason; expired and already-decided approvals answer with their `ErrorInfo.reason`.
- Watch streams send a snapshot, then changes, then heartbeats; a valid resume cursor replays what was missed, an unknown one answers with `snapshot_reset`.
- Pushes are sealed with HPKE to the device's key; the push log never contains plaintext.
- Browser sign-in checks PKCE S256 at the fake Authentik.

## Tests and fixtures

```sh
go test -race ./...
go run ./cmd/fixtures     # regenerates conformance/fixtures/push/sealed.json
```

The tests also check the shared golden fixtures in `../conformance/fixtures` (decision bytes, JWK thumbprints, the sealed notification), which the Android and iOS tests check too.

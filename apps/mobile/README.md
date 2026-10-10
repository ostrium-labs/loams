![Loams — Your data. Your bucket.](../../docs/assets/loams-banner.svg)

# Loams mobile

Native phone apps for [Loams](https://github.com/ostrium-labs/loams): **Loams for Android** (Kotlin, Jetpack Compose, connect-kotlin) and **Loams for iOS** (SwiftUI, connect-swift). Both talk Connect-RPC to a Loams instance, generated from the same protos the server uses. There is no web view, no bridge and no Kotlin Multiplatform.

The design is §37 of the main repository ([`docs/design/37-desktop-and-mobile-apps.md`](https://github.com/ostrium-labs/loams/blob/dev/docs/design/37-desktop-and-mobile-apps.md)); the plans are AP2 (Android) and AP3 (iOS).

> **Status: scaffold, mock-first.** Everything runs against the local mock server in [`mock/`](mock/). A real Loams server needs the unified auth plan (pairing grant, DPoP, the Authentik token exchange) and AP4 (the server side of the app protos). Nothing is published to a store yet.

## What the apps do

- **Pair** with a Loams instance by scanning a QR code (pinned TLS key and instance key), or **sign in** through Authentik with OIDC and PKCE.
- **Approve or reject** pending operations. Every decision is signed by a hardware key (Android Keystore or the Secure Enclave) that needs biometrics or the device passcode for each signature.
- **Watch** the instance and its operations live over a Connect server stream.
- **Receive push notifications** that are sealed end to end (HPKE), so Apple, Google and the push gateway see only ciphertext.

## Layout

| Path | What |
|---|---|
| `android/` | Gradle (Kotlin DSL) project: `:core`, `:proto`, `:transport`, `:conformance` (JVM) and `:data`, `:push`, `:app` (Android); see [docs/android.md](docs/android.md) |
| `ios/` | XcodeGen `project.yml`, the `Loams` app and its Notification Service Extension, Swift packages `LoamsCore`, `LoamsProto`, `LoamsData`; see [docs/ios.md](docs/ios.md) |
| `mock/` | A small Connect server in Go that serves the app protos for local testing |
| `proto/` | The protos, vendored from the main repository at the ref in `conformance/proto-ref.lock` |
| `conformance/` | The proto ref lock and golden fixtures shared by both apps and the mock |
| `docs/` | [Running the apps](docs/RUNNING.md), [protos and generation](docs/protos.md), [releasing](docs/release.md) |

## Quick start (Android, Linux or Windows)

```sh
cd mock && go run ./cmd/loams-mock          # terminal 1: the mock on 127.0.0.1:8084
cd android && ./gradlew installDebug        # terminal 2: with an emulator running (Windows: .\gradlew.bat)
```

Then tap **Debug: pair with the local mock** in the app. Full steps, including the emulator's screen lock and push: [docs/RUNNING.md](docs/RUNNING.md).

## Monorepo Nx and CI

Run mobile targets from the workspace root using `NX_DAEMON=false pnpm exec nx run <project>:<target>`.
Root `.github/workflows/monorepo.yml` owns active mobile CI:

| Nx project | Runs on | CI targets |
|---|---|---|
| `mobile-android` | Linux | `build`, `test`, `lint`, and separate mock-backed `conformance` |
| `mobile-ios` | macOS | `test` (package tests, Xcode generation, unsigned simulator tests) |
| `mobile-mock` | Linux | `vet`, race `test`, `build` |
| `mobile-contracts` | Linux | Buf `lint`, proto lock and generated-code `drift` |
| `mobile-fixtures` | — | Shared fixture/lock dependency inputs; no runnable targets |

The workflows nested under this directory are inactive historical templates; do not activate them alongside root CI. See [MIGRATION.md](MIGRATION.md) for all commands, dependency edges, integrated validation, and remaining platform limits. Validation needs no external keys. Store releases wait for the owner actions in [docs/release.md](docs/release.md).

## Licence

Apache License 2.0, the same as the main repository. See [LICENSE](LICENSE) and [NOTICE](NOTICE).

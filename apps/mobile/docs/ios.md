# Loams for iOS

Plan: AP3 in the main repository (`docs/plans/2026-10-01-ap3-ios-swiftui.md`); design §37 §7. The owner has no Mac, so this app is built and tested only in CI (`.github/workflows/ios.yml`, a `macos-latest` runner, unsigned, iOS simulator).

## Layout

| Path | What |
|---|---|
| `ios/project.yml` | XcodeGen spec; `xcodegen generate` writes `Loams.xcodeproj` (not committed) |
| `ios/Packages/LoamsCore` | Pure Swift (Swift 6 mode): pairing payload v1, reasons, canonical `DecisionClaims` + JWS, decision rules, watch state, `ResumingWatch` (resume, back-off, 45 s dead stream), HPKE unsealing, JWK thumbprints, PKCE. Builds on Linux with swift-crypto |
| `ios/Packages/LoamsProto` | Generated SwiftProtobuf + connect-swift code (committed; see [protos.md](protos.md)) |
| `ios/Packages/LoamsData` | Connect clients (URLSession, binary codec, GET reads), the token endpoint, instance identity check, SPKI pin validation, Secure Enclave decision keys behind LocalAuthentication, the keychain, the session store, push keys, `RemoteBackend` and `DemoBackend` |
| `ios/Loams` | SwiftUI app: welcome (VisionKit QR scanner, paste, typed code with fingerprint check, Authentik via `ASWebAuthenticationSession` with PKCE, demo), approvals, status, settings |
| `ios/LoamsNotificationService` | The Notification Service Extension: unseals and replaces the generic alert, fails closed |
| `ios/LoamsTests` | XCTest for the app model against `DemoBackend` |

Identifiers (AP3 Ruling 1): bundle `dev.loams.app`, extension `dev.loams.app.notification-service`, app group `group.dev.loams.app`, keychain group `<TEAM>.dev.loams.app.shared`, categories `APPROVAL` (action `REVIEW`), `OPERATION`, `JOB`, `SECURITY`.

## On a Mac

```sh
brew install xcodegen
cd ios && xcodegen generate && open Loams.xcodeproj
# in another terminal: cd mock && go run ./cmd/loams-mock -public-url http://localhost:8084
```

The simulator shares the Mac's network, so the app's default server is `http://localhost:8084` (Debug builds allow local networking under ATS; Release does not). The simulator has no Secure Enclave: there the decision key is a software key, and Face ID is enrolled from the simulator's Features menu.

## Rulings made during execution (AP3 Task 0)

| # | Ruling | Why |
|---|---|---|
| E1 | XcodeGen instead of a committed `.xcodeproj` or Tuist | Text-reviewable, one small tool |
| E2 | Generated code committed | Xcode builds need no build-phase script ([protos.md](protos.md)) |
| E3 | LoamsCore in Swift 6 mode; LoamsData and the app in Swift 5 mode for now | Security, LocalAuthentication and the generated code are not fully Sendable-annotated; moving to 6 is a follow-up |
| E4 | XCTest rather than swift-testing | Runs the same on Linux and in Xcode |
| E5 | Push sealing binds the ids through HPKE `info`, empty AAD | Matches Android (Tink) and the mock |

## Stubbed or not yet built

- **TLS pinning in the transport.** connect-swift's `URLSessionHTTPClient` keeps its session delegate private and forwards no auth challenges or redirects, so `PinValidator` is written and tested but not wired; it needs a small custom `HTTPClientInterface` (AP3 Task 3). The instance id and `jkt` checks are wired.
- **APNs.** `ApnsStub` returns a fake device token; real registration needs a signed build with `aps-environment` (Q420). The NSE reads push keys from the default keychain until the shared access group exists.
- **DPoP, the real pairing grant and the Authentik exchange** belong to the unified auth plan (Q438); they run against the mock today.
- **SwiftData cache, jobs and inbox screens, `BGAppRefreshTask`, multiple instances, XCUITest snapshots** (AP3 Tasks 5, 7, 8, 10).
- **Signing and TestFlight** (Q420).

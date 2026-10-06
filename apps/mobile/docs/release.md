# Releasing (not yet)

Nothing is published. These are the owner actions and open questions (from design §37 §15) that block a store release, and what each unblocks.

| Question | Owner action | Unblocks |
|---|---|---|
| **Q420** | An Apple Developer Program membership and a Google Play developer account for the `ostrium-labs` entity (D-U-N-S number, legal name); decide who holds the Play upload key and the Apple distribution identity | Android: a Firebase project (`google-services.json`) for real FCM, Play App Signing, the release job (signed AAB to internal testing), R8 rules. iOS: the team id in `ios/project.yml`, the shared keychain access group for the extension, real APNs tokens, provisioning profiles, TestFlight |
| **Q422** | Answered by the rename PR: proto packages are `loams.*` | — |
| **Q424** | Terms for the official push gateway `push.loams.dev` (free tier, limits, privacy policy, instance registration) | Push from self-hosted instances to the store apps |
| **Q431** | Confirm minimum OS versions: iOS 17, Android 10 (API 29) | Store listings |
| **Q433** | Crash reporting: none (proposed) or opt-in Sentry | Release builds |
| **Q434** | Store names and a trademark check | Store listings |
| **Q435** | App review: the bundled demo mode (built: "Try the demo") or a hosted demo instance | Store review |
| **Q439** | Whether and when to ship a `unifiedpush` flavor on F-Droid | The second Android flavor |

Also needed before a real server works with the apps, on the main repository's side: the unified auth plan (the pairing grant, the Authentik token exchange, DPoP; Q438), AP0's real protos and AP4 (the server side of the app services and `loams-push`).

## When the accounts exist

- **Android:** add the Firebase project, swap `FcmStubRegistrar` for `FirebaseMessaging`, add the `fcm` and `unifiedpush` flavors, R8 keep rules for protobuf-javalite, Tink and AppAuth, and a `release` job in `.github/workflows/android.yml` that runs only in a protected `release` environment with the upload key as an environment secret.
- **iOS:** set `DEVELOPMENT_TEAM`, the keychain access group `<TEAM>.dev.loams.app.shared` in `KeychainStore` for the app and the extension, register for remote notifications, and add signing (fastlane `match` or Xcode Cloud) and TestFlight upload to `.github/workflows/ios.yml`, again in a protected environment.
- **Both:** verified links (`assetlinks.json`, `apple-app-site-association`) for `https://loams.dev/app/auth/callback` (Q281), replacing the `dev.loams.app:/oauth2redirect` custom scheme.

# Running the apps

Everything runs against the local mock server in [`mock/`](../mock/README.md): there is no Loams server with the app APIs yet. The Android app runs on Linux, Windows or macOS; the iOS app needs a Mac (see [ios.md](ios.md)).

## Android: what you need

| | Linux | Windows |
|---|---|---|
| Android Studio (bundles a JDK and the SDK manager) | [developer.android.com/studio](https://developer.android.com/studio) | same |
| Android SDK Platform 37 and an emulator image (API 35 or 36, Google APIs, x86_64) | Android Studio's SDK Manager, or `sdkmanager "platforms;android-37" "system-images;android-36;google_apis;x86_64" emulator platform-tools` | same |
| Go 1.25 or later, for the mock (optional: CI publishes prebuilt binaries) | `sudo pacman -S go` / `sudo apt install golang-go` / [go.dev/dl](https://go.dev/dl/) | `winget install GoLang.Go` |

The project pins Gradle 9.8 through the wrapper, so no Gradle install is needed. Command-line builds need `ANDROID_HOME` set (or a `android/local.properties` with `sdk.dir=…`); Android Studio writes that file for you.

## 1. Start the mock

```sh
cd mock
go run ./cmd/loams-mock
```

It listens on `127.0.0.1:8084`, prints a pairing payload and a QR code, and logs what the app does. The Android emulator reaches your machine's loopback as `10.0.2.2`, which is the app's default address in debug builds. Leave it running.

No Go? Download the `loams-mock` artifact from the latest successful **mock** workflow run (repository → Actions → mock → the run → Artifacts) and run `loams-mock-linux-amd64` or `loams-mock-windows-amd64.exe`.

## 2. Prepare an emulator

1. Android Studio → Device Manager → create a device (for example Pixel 8, API 36, Google APIs).
2. Start it, then in the emulator: **Settings → Security → Screen lock → PIN**. The approval key needs a secure lock screen; pairing fails without one.
3. Optional, to approve with a fingerprint: **Settings → Security → Fingerprint**, and when it asks for a touch use the emulator's **⋯ (Extended controls) → Fingerprint → Touch sensor**. Without a fingerprint, the PIN works on API 30 and later.

## 3. Build and install the app

**Android Studio:** File → Open → the `android/` folder, wait for the Gradle sync, pick the emulator, Run ▶ `app`.

**Command line, Linux:**

```sh
cd android
./gradlew installDebug          # builds app-debug.apk and installs it on the running emulator
```

**Command line, Windows (PowerShell):**

```powershell
cd android
.\gradlew.bat installDebug
```

**Without building:** download `loams-android-debug-apk` from the latest successful **android** workflow run and install it with `adb install -r app-debug.apk`.

## 4. Try it

1. **Pair.** On the welcome screen tap **Debug: pair with the local mock at http://10.0.2.2:8084**. The app fetches a fresh pairing payload from the mock, checks the instance id and the instance key, creates its Keystore keys and redeems the code. Alternatives on the same screen: paste the payload the mock printed, use the 8-digit typed code (you are shown the key fingerprint to compare), or **Sign in** through the mock's fake Authentik (a Custom Tab opens and returns at once). **Try the demo** needs no mock at all.
2. **Approve.** On **Approvals**, open *Drop collection logs-2026 in production*, type `logs-2026`, tap **Approve**, and confirm with the PIN or the fingerprint. The mock verifies the ES256 signature over the canonical claims and the card disappears from the live list. *Create an API key…* was requested by you, so the mock refuses your approval; a reject needs a reason.
3. **Watch.** **Status** shows the instance and its operations over the `WatchOperations` server stream; the running one advances every 2 s. To see a reconnect, run `curl -X POST http://127.0.0.1:8084/mock/drop-streams`: the banner shows *Reconnecting* and the stream resumes from its cursor.
4. **Push.** In **Settings**, tap **Allow notifications** (Android 13+). The app registered a push target when it paired (the FCM token is a stub), and debug builds read the mock's push log while the app is open. Run `curl -X POST http://127.0.0.1:8084/mock/approvals`: within a few seconds a notification *Approval needed* appears. The mock sealed it with HPKE to the phone's key; the app unsealed it. Tap **Review** to open the approval.

**A USB phone instead of the emulator:** run `adb reverse tcp:8084 tcp:8084`, start the mock with `go run ./cmd/loams-mock -public-url http://127.0.0.1:8084`, and enter `http://127.0.0.1:8084` as the instance address on the welcome screen.

## 5. Tests

```sh
cd android
./gradlew test lint                     # JVM unit tests, the Robolectric Compose UI test, Android lint
./gradlew :core:test :transport:test    # the pure-JVM parts; no Android SDK needed
```

The conformance suite drives the app's network layer against a running mock on the JVM:

```sh
LOAMS_MOCK_URL=http://127.0.0.1:8084 ./gradlew :conformance:test
```

On Windows: `$env:LOAMS_MOCK_URL="http://127.0.0.1:8084"; .\gradlew.bat :conformance:test`. Without `LOAMS_MOCK_URL` these tests are skipped.

The mock's own tests: `cd mock && go test -race ./...`.

## iOS

Needs a Mac with Xcode; see [ios.md](ios.md). CI builds and tests it on a macOS runner for every change under `ios/`.

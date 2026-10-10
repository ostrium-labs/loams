# Third-party notices

No third-party source code is copied into this repository (see [NOTICE](NOTICE)). The apps and the mock depend on the libraries below, under their own licences. Generated code under `android/proto/src/generated`, `ios/Packages/LoamsProto/Sources/LoamsProto/Generated` and `mock/gen` is produced by the generators listed here from this project's protos.

## Android

| Library | Licence |
|---|---|
| AndroidX (Activity, Biometric, Core, DataStore, Fragment, Lifecycle), Jetpack Compose and Material 3 | Apache-2.0 |
| connect-kotlin (`com.connectrpc:connect-kotlin-okhttp`, `connect-kotlin-google-javalite-ext`) | Apache-2.0 |
| protobuf-javalite and the protobuf Java generator | BSD-3-Clause |
| OkHttp, Okio | Apache-2.0 |
| kotlinx.coroutines, kotlinx.serialization, Kotlin standard library | Apache-2.0 |
| Tink (`tink-android`) | Apache-2.0 |
| AppAuth for Android (`net.openid:appauth`) | Apache-2.0 |
| ZXing Android Embedded and ZXing core | Apache-2.0 |
| Tests: JUnit 4 (EPL-1.0, test only), Turbine (Apache-2.0), Robolectric (MIT), AndroidX Test (Apache-2.0) | as listed |

## iOS

| Library | Licence |
|---|---|
| connect-swift | Apache-2.0 |
| SwiftProtobuf and its generator | Apache-2.0 |

## Mock server (Go)

| Library | Licence |
|---|---|
| connect-go (`connectrpc.com/connect`) and its generator | Apache-2.0 |
| protobuf-go | BSD-3-Clause |
| CIRCL (`github.com/cloudflare/circl`) | BSD-3-Clause |
| qrterminal | MIT |
| rsc.io/qr | BSD-3-Clause |
| golang.org/x/crypto, x/sys, x/term | BSD-3-Clause |

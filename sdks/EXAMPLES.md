# Examples for all 13 Loams SDKs

Loams SDK **0.1.2** was released on October 26, 2026, and the `v0.1.2` source tag
is available. Registry availability varies by language: PHP is on Packagist;
Dart and Objective-C examples use the tagged source because their pub.dev and
CocoaPods packages are not published yet.

Ten languages have separate example repositories. PHP examples ship inside the
SDK and its generated split repository. Dart and Objective-C examples live in
this monorepo; neither has a separate example repository in `ostrium-labs`.

| SDK | App example in this repository | Separate example repository | Install at 0.1.2 |
| --- | --- | --- | --- |
| TypeScript | [notes](typescript/examples/notes/main.ts) | [loams-typescript](https://github.com/ostrium-labs/loams-typescript) | `@loams/client@0.1.2`, `@loams/live@0.1.2` |
| Python | [notes](python/examples/notes/main.py) | [loams-python](https://github.com/ostrium-labs/loams-python) | `loams==0.1.2` |
| Go | [notes](go/examples/notes/main.go) | [loams-go](https://github.com/ostrium-labs/loams-go) | `loams.dev/go@v0.1.2` |
| Rust | [notes](rust/examples/notes/main.rs) | [loams-rust](https://github.com/ostrium-labs/loams-rust) | `loams = "=0.1.2"` |
| C# | [notes](csharp/examples/Notes) | [loams-csharp](https://github.com/ostrium-labs/loams-csharp) | NuGet `Loams` version `0.1.2` |
| Java | [notes](java/examples/notes/Notes.java) | [loams-java](https://github.com/ostrium-labs/loams-java) | `dev.loams:loams:0.1.2` |
| Kotlin | [notes](kotlin/examples/notes/Notes.kt) | [loams-kotlin](https://github.com/ostrium-labs/loams-kotlin) | `dev.loams:loams-kotlin:0.1.2` |
| Swift | [notes](swift/Examples/Notes) | [loams-swift](https://github.com/ostrium-labs/loams-swift) | SwiftPM `exact: "0.1.2"` on `ostrium-labs/loams` |
| Ruby | [notes](ruby/examples/notes/notes.rb) | [loams-ruby](https://github.com/ostrium-labs/loams-ruby) | `gem "loams", "0.1.2"` |
| C++ | [notes](cpp/examples/notes/main.cpp) | [loams-cpp](https://github.com/ostrium-labs/loams-cpp) | `find_package(loams 0.1.2 EXACT CONFIG REQUIRED)` |
| PHP | [examples and run instructions](php/examples/README.md) | Included in the [generated SDK split](https://github.com/ostrium-labs/loams-php); not an examples-only repo | Packagist `loams/loams:0.1.2` |
| Dart | [notes and run instructions](dart/example/notes/README.md) | In this monorepo | Git dependency pinned to `v0.1.2`, path `sdks/dart` (pub.dev package pending) |
| Objective-C | [notes and integration instructions](objc/Examples/Notes/README.md) | In this monorepo | Local Swift package at `sdks/objc`, from the `v0.1.2` checkout (CocoaPods package pending) |

## Run source examples from the matching release

```sh
git clone --branch v0.1.2 --depth 1 https://github.com/ostrium-labs/loams.git
```

Follow each language's README for its toolchain and build commands. The app
examples use `LOAMS_ENDPOINT=http://127.0.0.1:8080` and a namespace supplied through
`LOAMS_NAMESPACE`. `LOAMS_API_KEY` is optional for a local engine with auth off.
Use a dedicated namespace for examples because they create and update data.

PHP's legacy service examples also accept `LOAMS_URL`; `LOAMS_ENDPOINT` takes
precedence. PHP's notes example uses the SDK's `LOAMS_ENDPOINT` setting.

## Where changes belong

Edit standalone example repositories for the first ten languages. Edit
`sdks/php`, `sdks/dart` and `sdks/objc` here for the other three. The PHP release
workflow generates `ostrium-labs/loams-php` from `sdks/php`; changes made directly
to that split would be overwritten by the next release.

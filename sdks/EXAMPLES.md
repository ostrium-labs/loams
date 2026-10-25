# Examples for all 13 Loams SDKs

These examples target the upcoming **0.1.2** release. Package installation and
release-tag checkouts require the matching packages and tags to be published.
SDK source versions are set separately by the release workflow.

Ten languages have separate example repositories. PHP examples ship inside the
SDK and its generated split repository. Dart and Objective-C examples live in
this monorepo; neither has a separate example repository in `ostrium-labs`.

| SDK | App example in this repository | Separate example repository | Exact 0.1.2 dependency |
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
| PHP | [examples and run instructions](php/examples/README.md) | Included in the [generated SDK split](https://github.com/ostrium-labs/loams-php); not an examples-only repo | `loams/loams:0.1.2` |
| Dart | [notes and run instructions](dart/example/notes/README.md) | In this monorepo | Dart `loams: 0.1.2` |
| Objective-C | [notes and integration instructions](objc/Examples/Notes/README.md) | In this monorepo | CocoaPods `pod 'Loams', '0.1.2'` (module `LoamsObjC`) |

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

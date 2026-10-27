# Dart notes example

This example targets Loams SDK **0.1.2**. It creates a namespace and
collection, writes notes, runs text/hybrid and direct vector search, SQL,
stream produce/subscribe, and counts documents. Graph is attempted only when
the server supports the experimental module.

Use Dart 3 and a running Loams engine. The `v0.1.2` source tag is published; run:

```sh
git clone --branch v0.1.2 --depth 1 https://github.com/ostrium-labs/loams
cd loams/sdks/dart
dart pub get
LOAMS_ENDPOINT=http://127.0.0.1:8080 LOAMS_NAMESPACE=examples-dart dart run example/notes/main.dart
```

Set `LOAMS_API_KEY` or `LOAMS_TOKEN` if the engine requires authentication.
The example uses session consistency so reads see its writes.

| Module family | Example coverage in SDK 0.1.2 |
|---|---|
| `instances` | Covered by startup API-version discovery. |
| `namespaces`, `collections`, `documents`, `search`, `vector`, `graph`, `sql`, `streams` | Covered by this app; graph is optional on experimental server builds. |
| Live, approvals, devices | SDK service APIs are available but not exercised in this app. |
| Authentication | Optional API key/token from environment; no sign-in or provisioning is implied. |
| Live admin, tables, system catalogue, House | No typed module in the Dart SDK 0.1.2. |

Running from `sdks/dart` uses the SDK source in that checkout; it does not
download the `loams` package from pub.dev. The 0.1.2 package is not currently
available on pub.dev. For a separate application, pin the monorepo package as
a Git dependency, then copy [`main.dart`](main.dart) into the application:

```yaml
dependencies:
  loams:
    git:
      url: https://github.com/ostrium-labs/loams.git
      ref: v0.1.2
      path: sdks/dart
```

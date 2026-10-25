# Dart notes example

This example targets upcoming Loams SDK **0.1.2**. It creates a namespace and
collection, writes notes, runs hybrid search and counts documents,
and demonstrates graph queries when the server supports them.

Use Dart 3 and a running Loams engine. After the `v0.1.2` source tag is
published, run:

```sh
git clone --branch v0.1.2 --depth 1 https://github.com/ostrium-labs/loams
cd loams/sdks/dart
dart pub get
LOAMS_ENDPOINT=http://127.0.0.1:8080 LOAMS_NAMESPACE=examples-dart dart run example/notes/main.dart
```

Set `LOAMS_API_KEY` or `LOAMS_TOKEN` if the engine requires authentication.
The example uses session consistency so reads see its writes.

Running from `sdks/dart` uses the SDK source in that checkout; it does not
download the `loams` package from pub.dev. For a separate application, install
the exact published dependency with `dart pub add loams:0.1.2`, then copy
[`main.dart`](main.dart) into the application. That installation requires
the 0.1.2 package to be published. The source tag and package may be unavailable
until the release is published.

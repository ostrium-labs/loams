# Objective-C notes example

This example uses instance discovery, creates a namespace and a `notes` collection,
upserts documents, then runs hybrid and direct vector search, parameterized SQL,
and stream produce/subscribe. It targets Loams SDK 0.1.3; graph remains optional
because the example server may not carry the experimental module.

## Set up

You need macOS 12 or newer and Xcode. The CocoaPods package is not published;
use the released source instead. Clone the monorepo tag and add its Objective-C
SDK package to your Xcode project:

```sh
git clone --branch v0.1.3 --depth 1 https://github.com/ostrium-labs/loams.git
```

In Xcode, create a macOS Command Line Tool project named `Notes` with Objective-C
as its language and set the deployment target to macOS 12 or newer. Choose
**File → Add Package Dependencies → Add Local…** and select
`loams/sdks/objc`; add its `LoamsObjC` product to the `Notes` target. Add this
directory's [main.m](main.m) to the target. The example imports the module with
`@import LoamsObjC;`.

## Run

Start a local engine with `loams dev`. In Xcode's **Product → Scheme → Edit
Scheme → Run → Arguments**, add these environment variables:

| Variable | Value |
| --- | --- |
| `LOAMS_ENDPOINT` | `http://127.0.0.1:8080` |
| `LOAMS_NAMESPACE` | `examples-objc` |

Set `LOAMS_API_KEY` if your engine requires authentication. Build and run the
`Notes` scheme. The example prints matching document keys and titles to the
console and writes its sample data into the `examples-objc` namespace.

You can also run the executable produced by Xcode:

```sh
LOAMS_ENDPOINT=http://127.0.0.1:8080 LOAMS_NAMESPACE=examples-objc /path/to/Notes
```

| Module family | Example coverage in SDK 0.1.3 |
|---|---|
| `instance` | Covered by startup discovery. |
| `namespaces`, `collections`, `documents`, `search`, `vector`, `sql`, `streams` | Covered by this app. |
| `graph` | SDK module available; not invoked here because server support is experimental and optional. |
| Live, approvals, devices, Live admin, tables | No typed service/module accessors on the Objective-C client; its generic descriptor-backed `call` is available but not used by this app. |
| Authentication | Optional environment API key/token; no sign-in or provisioning is implied. |
| System catalogue, House | No typed module in the Objective-C SDK 0.1.3. |

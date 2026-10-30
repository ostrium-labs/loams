# Loams Kotlin SDK (preview)

> **Pre-release (preview).** This SDK is published as a preview of Loams. Its API
> can change between previews without notice: pin the exact version, and expect
> breaking changes until 1.0. Report problems at
> <https://github.com/ostrium-labs/loams/issues>.

## Install

Gradle (Kotlin DSL):

```kotlin
dependencies {
    implementation("dev.loams:loams-kotlin:0.1.4")
}
```

Maven:

```xml
<dependency>
  <groupId>dev.loams</groupId>
  <artifactId>loams-kotlin</artifactId>
  <version>0.1.4</version>
</dependency>
```

It runs on Java 11 or newer, and its only runtime dependencies are
`com.google.protobuf:protobuf-java` and the Kotlin standard library. The Java SDK
is a separate artifact, `dev.loams:loams`; use one or the other.

## Build an app

Namespaces, collections, documents, search, vector and graph, with no REST calls
(SDKP). [`examples/notes/Notes.kt`](examples/notes/Notes.kt) is the whole notes app;
`LOAMS_ENDPOINT=http://127.0.0.1:8080 ./build.sh example` runs it.

```kotlin
// LOAMS_ENDPOINT, LOAMS_API_KEY or LOAMS_TOKEN, LOAMS_NAMESPACE; the session gives reads
// read-your-writes (R4); a request with no namespace gets LOAMS_NAMESPACE (R11).
val loams = LoamsClient.fromEnv(session = ConsistencySession())

loams.namespaces.create("app")
loams.collections.create("notes", Schema().text("title").text("body").keyword("tags").i64("n").vector("embedding", 4))

val notes = loams.collection("notes")
notes.upsert(listOf(Doc(1, mapOf("title" to "Refund policy", "tags" to listOf("policy"), "n" to 1),
    mapOf("embedding" to floatArrayOf(1f, 0f, 0f, 0f)))))

val hits = loams.search.query(search("notes") {
    text("refund", "body")
    vector("embedding", floatArrayOf(1f, 0f, 0f, 0f))
    fuse(rrf(60))                                   // or dbsf(), weighted(0.7, 0.3)
    filter(Filter.eq("tags", "policy") and Filter.range("n", gte = 1))
    highlight("body")
    limit(5)
})

val policies = notes.count(Filter.eq("tags", "policy"))
for (doc in notes.scrollAll(limit = 100)) { /* every document, in id order */ }
val batch = notes.writeBatch(manyDocs)               // chunks of 10 000 ops, one key per chunk (R13)
loams.graph.query("notes_graph", "INSERT (:Note {id: \$id})", mapOf("id" to GraphModule.Param.int(1)))   // experimental
```

Modules: `namespaces`, `collections`, `documents`, `search` (`query`, `queryAll` with a
`Sort.pk()` tiebreaker, `hybrid`), `vector` and `graph` (experimental), typed wrappers
over the descriptor-driven calls. A request message can always be passed as JSON
through `Messages.of(type, json)`. Calls answer `DynamicMessage`; `field`, `text` and
`messages` read it. Deadlines are 30 s per unary call by default (`LoamsClient(timeout =
…)`, `CallOptions(timeout = …)`), and a spent one raises `DeadlineExceededException`
with reason `client_deadline_exceeded`. SQL and streams arrive with `loams.sql.v1` and
`loams.stream.v1`; `Subscription` is the resumable fetch loop the streams module will
use. Durable has no module: use the [Resonate SDK](https://docs.resonatehq.io) with
`LOAMS_ENDPOINT/durable/` as its URL.

## Develop

```sh
./build.sh test              # compile and run the suite (needs a JDK and kotlinc)
./build.sh package OUT_DIR   # the jars and POM sdk-release.yml publishes
```

Licensed under Apache-2.0 (see `LICENSE`).

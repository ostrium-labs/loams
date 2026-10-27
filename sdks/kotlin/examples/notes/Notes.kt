// A notes app on Loams with nothing but the Kotlin SDK: a namespace, a collection
// with a schema, five notes, reads that see their own writes, text, vector and
// hybrid search with a filter, a count, a scroll, and a graph over the notes.
//
//     ./build.sh example     # with LOAMS_ENDPOINT and LOAMS_NAMESPACE set
//
// LOAMS_API_KEY (or LOAMS_TOKEN) is sent as the bearer when it is set.

import com.google.protobuf.DynamicMessage
import dev.loams.AlreadyExistsException
import dev.loams.ConsistencySession
import dev.loams.ProducerRecord
import dev.loams.Doc
import dev.loams.DocumentId
import dev.loams.Filter
import dev.loams.GraphModule
import dev.loams.LoamsClient
import dev.loams.NotFoundException
import dev.loams.Schema
import dev.loams.Sort
import dev.loams.UnimplementedException
import dev.loams.dbsf
import dev.loams.field
import dev.loams.messages
import dev.loams.rrf
import dev.loams.search
import dev.loams.text
import dev.loams.weighted

private fun ids(response: DynamicMessage): String =
    response.messages("hits").joinToString(", ") { DocumentId.of(it.field("pk") as DynamicMessage)?.text ?: "?" }

private fun note(id: Long, title: String, body: String, tags: List<String>, embedding: FloatArray) =
    Doc(id, mapOf("title" to title, "body" to body, "tags" to tags, "n" to id), mapOf("embedding" to embedding))

fun main() {
    val ns = System.getenv("LOAMS_NAMESPACE") ?: "notes-demo"
    val loams = LoamsClient.fromEnv(System.getenv() + ("LOAMS_NAMESPACE" to ns), session = ConsistencySession())
    val instance = loams.getInstance()
    println("connected to ${instance.field("name")} ${instance.field("server_version")}")

    // 1. A namespace (a repeat is already_exists) and a collection.
    try {
        loams.namespaces.create(ns)
    } catch (_: AlreadyExistsException) {
    }
    loams.collections.create("notes", Schema().text("title").text("body").keyword("tags").i64("n").vector("embedding", 4))
    val notes = loams.collection("notes")

    // 2. Five notes, as one write; the session gives the reads below read-your-writes.
    val write = notes.upsert(listOf(
        note(1, "Refund policy", "Refunds are issued within 14 days.", listOf("policy", "billing"), floatArrayOf(1f, 0f, 0f, 0f)),
        note(2, "Shipping", "Orders ship in two business days.", listOf("shipping"), floatArrayOf(0f, 1f, 0f, 0f)),
        note(3, "Refund window", "The refund window closes after 30 days.", listOf("policy"), floatArrayOf(0.9f, 0.1f, 0f, 0f)),
        note(4, "Support hours", "Support answers on weekdays.", listOf("support"), floatArrayOf(0f, 0f, 1f, 0f)),
        note(5, "Returns", "Returns need a receipt and a refund form.", listOf("policy", "returns"), floatArrayOf(0.7f, 0f, 0.3f, 0f)),
    ))
    println("wrote 5 notes, token ${write.text("token")}")
    println("read back ${notes.get(listOf(1, 3)).messages("documents").size} notes")

    notes.patch(2, mapOf("title" to "Shipping times"))
    notes.delete(listOf(4))
    println("${notes.count(Filter.eq("tags", "policy"))} notes tagged policy")
    for (doc in notes.scrollAll(limit = 2)) {
        val id = DocumentId.of(doc.field("id") as DynamicMessage)?.text
        val source = doc.field("source") as DynamicMessage
        println("  note $id: ${dev.loams.MessageCodec.serialize(source, dev.loams.Codec.JSON).toString(Charsets.UTF_8)}")
    }

    // 3. Search: text with a filter, highlights and a projection; a vector; hybrid.
    val text = loams.search.query(search("notes") {
        text("refund", "body"); filter(Filter.eq("tags", "policy")); highlight("body", pre = "[", post = "]")
        select(include = listOf("title")); sort(Sort.score(), Sort.pk()); limit(2)
    })
    println("text: ${ids(text)}")
    println("vector: ${ids(loams.vector.search("notes", floatArrayOf(1f, 0f, 0f, 0f), "embedding", limit = 3))}")
    for ((name, fusion) in listOf("rrf" to rrf(60), "dbsf" to dbsf(), "weighted" to weighted(0.7, 0.3))) {
        val hybrid = loams.search.query(search("notes") {
            text("refund", "body"); vector("embedding", floatArrayOf(1f, 0f, 0f, 0f)); fuse(fusion); limit(3)
        })
        println("hybrid $name: ${ids(hybrid)}")
    }

    // 4. A graph over the notes (experimental; skipped on a build without it).
    try {
        try {
            loams.graph.create("notes_graph")
        } catch (_: AlreadyExistsException) {
        }
        loams.graph.query("notes_graph", "INSERT (:Note {id: \$id, title: \$title})-[:TAGGED]->(:Tag {name: \$tag})",
            mapOf("id" to GraphModule.Param.int(1), "title" to GraphModule.Param.string("Refund policy"),
                "tag" to GraphModule.Param.string("policy")))
        val rows = loams.graph.query("notes_graph", "MATCH (n:Note)-[:TAGGED]->(t:Tag) RETURN n.title, t.name", readOnly = true)
        println("graph: ${(rows.field("rows") as DynamicMessage).messages("rows").size} row(s)")
        loams.graph.delete("notes_graph")
    } catch (_: UnimplementedException) {
        println("graph: not in this build")
    }

    // 5. SQL with $n parameters over the same collection, and a stream of events.
    val sql = loams.sql.query("SELECT n, title FROM notes WHERE n > \$1 ORDER BY n", listOf(1))
    for (row in dev.loams.SqlModule.records(sql)) println("  sql row ${row["n"]}: ${row["title"]}")
    loams.streams.create("events", 1)
    val produced = loams.streams.produce("events", 0, listOf(
        ProducerRecord("1", mapOf("kind" to "created", "note" to 1)),
        ProducerRecord("2", mapOf("kind" to "patched", "note" to 2)),
        ProducerRecord("4", mapOf("kind" to "deleted", "note" to 4)),
    ), producerId = "notes-demo", producerSeq = 1)
    val base = produced.field("base_offset") as Long
    val last = produced.field("last_offset") as Long
    println("events at $base..$last${if (produced.field("duplicate") == true) " (already produced)" else ""}")
    loams.streams.subscribe("events", 0, fromOffset = base).use { subscription ->
        for (record in subscription) {
            println("  event ${record.offset}: ${record.text}")
            if (record.offset >= last) break
        }
    }

    // 6. A missing collection is a typed error; then clean up.
    try {
        loams.collections.get("missing")
    } catch (missing: NotFoundException) {
        println("missing collection: ${missing.code}")
    }
    notes.drop()
    println("done")
}

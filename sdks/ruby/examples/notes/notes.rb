# frozen_string_literal: true

# A notes app on Loams: a namespace, a collection with a schema, documents,
# a hybrid search with a filter, and a graph query.
#
#   LOAMS_ENDPOINT=http://127.0.0.1:8080 LOAMS_NAMESPACE=app ruby -Ilib examples/notes/notes.rb
require "loams"

client = Loams.from_env(session_consistency: true)

begin
  client.namespaces.create(client.namespace)
rescue Loams::AlreadyExistsError
  # Created by an earlier run.
end

schema = Loams::Schema.new
  .keyword("title")
  .text("body")
  .keyword("tags")
  .i64("n")
  .vector("embedding", 4)
begin
  client.collections.create("notes", schema: schema)
rescue Loams::AlreadyExistsError
  # Created by an earlier run.
end

notes = client.collection("notes")
notes.upsert([
  { id: 1, source: { title: "Refunds", body: "refund policy for orders", tags: ["policy"], n: 1 }, vectors: { embedding: [1, 0, 0, 0] } },
  { id: 2, source: { title: "Shipping", body: "shipping times", tags: ["ops"], n: 2 }, vectors: { embedding: [0, 1, 0, 0] } },
  { id: 3, source: { title: "Refund window", body: "the refund window is thirty days", tags: ["policy"], n: 3 }, vectors: { embedding: [0.9, 0.1, 0, 0] } },
])

# With session_consistency, this read sees the write above.
search = Loams::Search.on("notes")
  # k is explicit so the example also runs on a 0.1.0 engine, which has no default k.
  .text("refund", fields: ["body"], k: 10)
  .vector("embedding", [1, 0, 0, 0], k: 10)
  .fuse(Loams::Fusion.rrf(60))
  .filter(Loams::Filter.eq("tags", "policy"))
  .highlight("body")
  .select(include: ["title"])
  .limit(5)
notes.search(search).hits.each do |hit|
  puts "#{Loams::Values.from_document_id(hit.pk)}: #{Loams.to_h(hit.source)["title"]} (#{hit.score.round(3)})"
end

puts "#{notes.count.count} notes"

# Graph is experimental and not in every build.
begin
  client.graph.create("notes_graph")
  client.graph.query("notes_graph", "INSERT (:Note {id: $id})-[:TAGGED]->(:Tag {name: $tag})", parameters: { id: 1, tag: "policy" })
  rows = client.graph.query("notes_graph", "MATCH (n:Note)-[:TAGGED]->(t:Tag) RETURN n.id AS id, t.name AS tag", read_only: true)
  p Loams::App::Graph.records(rows)
  client.graph.delete("notes_graph")
rescue Loams::FeatureNotInVariantError
  puts "this server has no graph"
end

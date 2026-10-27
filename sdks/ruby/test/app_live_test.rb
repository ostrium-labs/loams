# frozen_string_literal: true

require_relative "test_helper"

# The SDKP app scenario (`sdks/fixtures/app-scenario.md`) through the public
# facade, against a running server: `LOAMS_TEST_ENDPOINT=http://… rake test`.
# Skipped without one. Until P0d records the scenario for the fixture server,
# this is how the app modules are checked end to end (`loams dev`).
class AppLiveTest < Minitest::Test
  NOTES = [
    { id: 1, source: { title: "Refunds", body: "refund policy for orders", tags: ["policy"], n: 1 }, vectors: { embedding: [1.0, 0.0, 0.0, 0.0] } },
    { id: 2, source: { title: "Shipping", body: "shipping times and carriers", tags: ["ops"], n: 2 }, vectors: { embedding: [0.0, 1.0, 0.0, 0.0] } },
    { id: 3, source: { title: "Refund window", body: "the refund window is thirty days", tags: ["policy"], n: 3 }, vectors: { embedding: [0.9, 0.1, 0.0, 0.0] } },
    { id: 4, source: { title: "Returns", body: "returns need a receipt", tags: ["policy", "ops"], n: 4 }, vectors: { embedding: [0.5, 0.5, 0.0, 0.0] } },
    { id: 5, source: { title: "Holidays", body: "holiday hours", tags: ["ops"], n: 5 }, vectors: { embedding: [0.0, 0.0, 1.0, 0.0] } },
  ].freeze

  def setup
    @endpoint = ENV["LOAMS_TEST_ENDPOINT"]
    skip "LOAMS_TEST_ENDPOINT is not set" if @endpoint.nil? || @endpoint.empty?
    @ns = "parity-ruby-#{Process.clock_gettime(Process::CLOCK_REALTIME, :millisecond)}-#{rand(1000)}"
    @client = Loams::Client.new(@endpoint, namespace: @ns, session_consistency: true, api_key: ENV["LOAMS_TEST_API_KEY"])
  end

  def schema
    Loams::Schema.new.keyword("title").text("body").keyword("tags").i64("n").vector("embedding", 4).dynamic("ignore")
  end

  def seed
    @client.namespaces.create(@ns)
    @client.collections.create("notes", schema: schema)
    @client.documents.upsert("notes", NOTES)
  end

  def test_ruby_live_app_collections
    instance = @client.instance.get_instance
    refute_nil instance

    @client.namespaces.create(@ns)
    assert_raises(Loams::AlreadyExistsError) { @client.namespaces.create(@ns) }
    assert_includes @client.namespaces.list_all.map(&:namespace), @ns

    info = @client.collections.create("notes", schema: schema)
    assert_equal "notes", info.name
    assert_equal "notes", @client.collections.get("notes").name
    @client.collections.create("scratch", schema: Loams::Schema.new.keyword("k"))
    names = @client.collections.list_all(page_size: 1).map(&:name)
    assert_equal %w[notes scratch], names.sort

    key = Loams::Idempotency.uuidv7
    first = @client.documents.upsert("notes", NOTES, idempotency_key: key)
    again = @client.documents.upsert("notes", NOTES, idempotency_key: key)
    assert_equal first.token, again.token, "the same key answers the same write (R3)"

    got = @client.documents.get("notes", [1, 2], consistency: first.token)
    assert_equal %w[Refunds Shipping], got.documents.map { |d| Loams.to_h(d.source)["title"] }

    @client.documents.patch("notes", 4, set: { title: "Returns and exchanges" })
    @client.documents.delete("notes", [5])
    # The native filter JSON until P0a types `where`.
    assert_equal 3, @client.documents.count("notes", filter: { "term" => { "field" => "tags", "value" => "policy" } }).count

    all = @client.documents.scroll_all("notes", limit: 2).map { |d| Loams::Values.from_document_id(d.id) }
    assert_equal [1, 2, 3, 4], all.sort
    patched = @client.documents.get("notes", [4]).documents.first
    assert_equal "Returns and exchanges", Loams.to_h(patched.source)["title"]

    err = assert_raises(Loams::NotFoundError) { @client.collections.get("missing") }
    assert_equal :not_found, err.code
    assert @client.collections.drop("scratch").dropped

    handle = @client.collection("notes")
    assert_equal "notes", handle.info.name
    result = handle.write_batch((10..14).map { |i| { id: i, source: { title: "b#{i}", n: i } } }, chunk_rows: 2)
    assert_equal [3, 5], [result.chunks, result.rows]
    assert_equal 9, handle.count(consistency: result.token).count
  end

  def test_ruby_live_app_search
    seed
    f = Loams::Filter
    s = Loams::Search
    page = @client.search.query(
      s.on("notes").text("refund", fields: ["body"]).filter(f.eq("tags", "policy"))
        .highlight("body").select(include: ["title"]).limit(2).sort(:score, :pk),
    )
    assert_equal 2, page.hits.size
    assert_equal({ "title" => page.hits.first.source.fields["title"].string_value }, Loams.to_h(page.hits.first.source))
    refute_empty page.hits.first.highlight.to_h

    all = @client.search.query_all(s.on("notes").text("refund", fields: ["body"]).sort(:score, :pk).limit(1)).to_a
    assert_equal [1, 3], all.map { |h| Loams::Values.from_document_id(h.pk) }.sort

    near = @client.vector.search("notes", [1, 0, 0, 0], field: "embedding", limit: 2)
    assert_equal 1, Loams::Values.from_document_id(near.hits.first.pk)

    [Loams::Fusion.rrf(60), Loams::Fusion.dbsf, Loams::Fusion.weighted(0.7, 0.3)].each do |fusion|
      res = @client.search.hybrid("notes", "refund", [1, 0, 0, 0], field: "embedding", fields: ["body"],
        fusion: fusion, filter: f.eq("tags", "policy"), limit: 3)
      refute_empty res.hits, fusion.keys.first
    end
    err = assert_raises(Loams::InvalidArgumentError) do
      @client.search.hybrid("notes", "refund", [1, 0, 0, 0], field: "embedding", fields: ["body"], fusion: Loams::Fusion.weighted(0.7))
    end
    assert_equal :invalid_argument, err.code

    hits = @client.collection("notes").search { |q| q.text("refund", fields: ["body"]).limit(5) }.hits
    assert_equal 2, hits.size
  end

  def test_ruby_live_app_graph
    @client.namespaces.create(@ns)
    begin
      @client.graph.create("notes_graph")
    rescue Loams::FeatureNotInVariantError, Loams::UnimplementedError
      skip "this build serves no graph"
    end
    write = @client.graph.query("notes_graph",
      "INSERT (:Note {id: $id, title: $title})-[:TAGGED]->(:Tag {name: $tag})",
      parameters: { id: 1, title: "Refunds", tag: "policy" })
    refute_nil write
    read = @client.graph.query("notes_graph", "MATCH (n:Note)-[:TAGGED]->(t:Tag) RETURN n.title AS title, t.name AS tag", read_only: true)
    assert_equal [{ "title" => "Refunds", "tag" => "policy" }], Loams::App::Graph.records(read)
    assert_includes @client.graph.list_all.map(&:name), "notes_graph"
    refute_nil @client.graph.delete("notes_graph")
  end
end

# frozen_string_literal: true

require_relative "test_helper"

# The app-building API without a server (SDKP Part A): the token corpus, the
# runtime additions R11–R14, the builders and the collection handle.
class AppTest < Minitest::Test
  RPC = Loams::App::CALLS

  def binding(path)
    Loams::Descriptors.binding_for_rpc(path)
  end

  def stub_client(tp, **opts)
    Loams::Client.new("http://127.0.0.1:4400", transport: tp, **opts)
  end

  def sent(tp, path, index = -1)
    binding(path).request.msgclass.decode(tp.requests[index].body)
  end

  def answer(tp, path, json = {})
    b = binding(path)
    tp.answer_message(b.response, b.response.msgclass.decode_json(JSON.generate(json)))
  end

  # --- R4: tokens and the session -----------------------------------------

  # The token corpus (`sdks/fixtures/tokens.json`), and the cases of
  # `crates/loams-collection/src/token.rs` it was taken from.
  def test_ruby_consistency_token_merge
    t = Loams::ConsistencyToken
    assert_equal "v1:s7/p3@918274", t.parse("v1:s7/p3@918274").to_s
    assert_equal "v1:s2/p0@1,s1/p5@3", t.parse("v1:s2/p0@1,s1/p5@3").to_s, "parse keeps the order"
    max = "v1:s18446744073709551615/p4294967295@18446744073709551615"
    assert_equal max, t.parse(max).to_s
    assert t.parse("v1:").empty?

    merged = t.parse("v1:s7/p3@10,s1/p0@5").merge("v1:s7/p3@4,s7/p1@2,s1/p0@9")
    assert_equal "v1:s1/p0@9,s7/p1@2,s7/p3@10", merged.to_s
    assert_equal 10, merged.offset(7, 3)
    assert_nil merged.offset(1, 1)
    assert_equal "v1:s1/p0@9007199254740993", t.parse("v1:s1/p0@9007199254740993").merge("v1:s1/p0@9007199254740992").to_s
    assert_equal t.parse("v1:s1/p0@1"), t.parse("v1:s1/p0@1")

    corpus = File.join(Loams::Test::BuilderCorpus.dir, "tokens.json")
    skip "no token corpus at #{corpus}" unless File.exist?(corpus)

    data = JSON.parse(File.read(corpus))
    data.fetch("parse").each do |c|
      parsed = t.parse(c["text"])
      assert_equal c["text"], parsed.to_s, c["name"]
      assert_equal c["items"].map { |s, p, o| [Integer(s), Integer(p), Integer(o)] }, parsed.items, c["name"]
    end
    data.fetch("merge").each do |c|
      assert_equal c["expect"], t.parse(c["a"]).merge(t.parse(c["b"])).to_s, c["name"]
    end
    data.fetch("malformed").each do |bad|
      err = assert_raises(Loams::InvalidArgumentError, bad.inspect) { t.parse(bad) }
      assert_equal :invalid_argument, err.code
    end
  end

  def test_ruby_session_read_your_writes
    tp = Loams::Test::StubTransport.new
    answer(tp, RPC[%w[documents write]], { "token" => "v1:s1/p0@5" })
    answer(tp, RPC[%w[documents get]], { "readToken" => "v1:s1/p0@5,s1/p1@3" })
    answer(tp, RPC[%w[documents get]], {})
    answer(tp, RPC[%w[documents get]], {})
    client = stub_client(tp, namespace: "app", session_consistency: true)

    client.documents.upsert("notes", [{ id: 1, source: { title: "a" } }])
    client.documents.get("notes", [1])
    assert_equal "v1:s1/p0@5", sent(tp, RPC[%w[documents get]], 1).consistency.at_least
    client.documents.get("notes", [1])
    assert_equal "v1:s1/p0@5,s1/p1@3", sent(tp, RPC[%w[documents get]], 2).consistency.at_least

    # An explicit consistency is the caller's and is never overwritten.
    client.documents.get("notes", [1], consistency: :eventual)
    got = sent(tp, RPC[%w[documents get]], 3).consistency
    assert_equal :FRESHNESS_EVENTUAL, got.freshness
    assert_equal "", got.at_least

    # SDKP-17: a graph read never takes the session's token.
    answer(tp, RPC[%w[graph query]], {})
    client.graph.query("g", "MATCH (n) RETURN n", read_only: true)
    assert_nil sent(tp, RPC[%w[graph query]], 4).consistency

        # Off by default: no session, nothing attached.
    tp2 = Loams::Test::StubTransport.new
    answer(tp2, RPC[%w[documents write]], { "token" => "v1:s1/p0@5" })
    answer(tp2, RPC[%w[documents get]], {})
    plain = stub_client(tp2, namespace: "app")
    plain.documents.upsert("notes", [{ id: 1, source: {} }])
    plain.documents.get("notes", [1])
    assert_nil sent(tp2, RPC[%w[documents get]]).consistency
  end

  # --- R11: the default namespace ------------------------------------------

  def test_ruby_default_namespace
    tp = Loams::Test::StubTransport.new
    answer(tp, RPC[%w[collections get]], { "name" => "notes" })
    answer(tp, RPC[%w[collections get]], { "name" => "notes" })
    client = stub_client(tp, namespace: "app")
    client.collections.get("notes")
    assert_equal "app", sent(tp, RPC[%w[collections get]], 0).namespace
    client.collections.get("notes", namespace: "other")
    assert_equal "other", sent(tp, RPC[%w[collections get]], 1).namespace

    bare = Loams::Test::StubTransport.new
    err = assert_raises(Loams::InvalidArgumentError) { stub_client(bare).collections.get("notes") }
    assert_equal :invalid_argument, err.code
    assert_equal :namespace_required, err.reason
    assert_empty bare.requests, "refused before any network call"
  end

  # --- R12: timeouts ---------------------------------------------------------

  class TimeoutTransport < Loams::Test::StubTransport
    def call_unary(request)
      super
      raise Net::ReadTimeout
    end
  end

  def test_ruby_timeout_deadline
    tp = Loams::Test::StubTransport.new
    answer(tp, RPC[%w[collections get]], {})
    answer(tp, RPC[%w[collections get]], {})
    client = stub_client(tp, namespace: "app", timeout: 5)
    client.collections.get("notes")
    ms = Integer(tp.requests[0].headers["connect-timeout-ms"])
    assert ms.positive? && ms <= 5000, "the client default is sent as connect-timeout-ms"
    client.collections.get("notes", timeout: 0.25)
    assert_operator Integer(tp.requests[1].headers["connect-timeout-ms"]), :<=, 250

    slow = TimeoutTransport.new
    slow.answer(200, "application/proto", "")
    slow.answer(200, "application/proto", "")
    err = assert_raises(Loams::DeadlineExceededError) do
      stub_client(slow, namespace: "app", max_retries: 3).collections.get("notes")
    end
    assert_equal :deadline_exceeded, err.code
    assert_equal :client_deadline_exceeded, err.reason
    assert_equal 1, slow.requests.size, "a spent budget is never retried"

    none = Loams::Test::StubTransport.new
    answer(none, RPC[%w[collections get]], {})
    stub_client(none, namespace: "app", timeout: nil).collections.get("notes")
    assert_nil none.requests[0].headers["connect-timeout-ms"]
  end

  # --- R13: write_batch ------------------------------------------------------

  def test_ruby_write_batch_chunks
    tp = Loams::Test::StubTransport.new
    answer(tp, RPC[%w[documents write]], { "token" => "v1:s1/p0@2" })
    answer(tp, RPC[%w[documents write]], { "token" => "v1:s1/p1@2" })
    answer(tp, RPC[%w[documents write]], { "token" => "v1:s1/p0@5" })
    client = stub_client(tp, namespace: "app")
    docs = (1..5).map { |i| { id: i, source: { n: i } } }
    result = client.documents.write_batch("notes", docs, chunk_rows: 2)
    assert_equal "v1:s1/p0@5,s1/p1@2", result.token
    assert_equal 3, result.chunks
    assert_equal 5, result.rows
    sizes = tp.requests.map { |r| binding(RPC[%w[documents write]]).request.msgclass.decode(r.body).ops.size }
    assert_equal [2, 2, 1], sizes
    keys = tp.requests.map { |r| r.headers["idempotency-key"] }
    assert_equal 3, keys.uniq.size, "every chunk has its own key"

    # A retried chunk reuses its key; a failed chunk stops the batch.
    tp2 = Loams::Test::StubTransport.new
    answer(tp2, RPC[%w[documents write]], { "token" => "v1:s1/p0@2" })
    tp2.answer_connect_error(503, "unavailable", "down")
    answer(tp2, RPC[%w[documents write]], { "token" => "v1:s1/p0@4" })
    tp2.answer_connect_error(400, "invalid_argument", "bad doc")
    err = assert_raises(Loams::BatchError) do
      stub_client(tp2, namespace: "app").documents.write_batch("notes", docs, chunk_rows: 2)
    end
    assert_equal 2, err.chunks_applied
    assert_equal 4, err.rows_applied
    assert_equal "v1:s1/p0@4", err.token
    assert_equal "2", err.metadata["chunks_applied"]
    assert_equal :batch_partially_applied, err.reason
    assert_equal :aborted, err.code
    assert_equal :invalid_argument, err.cause_error.code
    assert_equal tp2.requests[1].headers["idempotency-key"], tp2.requests[2].headers["idempotency-key"]

    # The byte budget closes a chunk too.
    tp3 = Loams::Test::StubTransport.new
    5.times { answer(tp3, RPC[%w[documents write]], {}) }
    stub_client(tp3, namespace: "app").documents.write_batch("notes", docs, chunk_bytes: 60)
    assert_operator tp3.requests.size, :>, 1
  end

  # --- R14: subscribe ---------------------------------------------------------

  def test_ruby_streams_subscribe_resumes
    log = (0..4).map { |i| { "offset" => i, "value" => "r#{i}" } }
    fetched = []
    sleeps = []
    sub = Loams::Subscription.new(from_offset: 1, sleeper: ->(s) { sleeps << s }) do |offset|
      fetched << offset
      fetched.size == 2 || fetched.size == 3 ? [] : log.select { |r| r["offset"] >= offset }.first(2)
    end
    got = []
    sub.each do |record|
      got << record["value"]
      sub.cancel if got.size == 4
    end
    assert_equal %w[r1 r2 r3 r4], got
    assert_equal 5, sub.offset, "the next offset is the last offset + 1"
    assert_equal [1, 3, 3, 3], fetched
    assert_equal [0.1, 0.2], sleeps, "an empty fetch backs off, doubling"

    resumed = Loams::Subscription.new(from_offset: sub.offset, sleeper: ->(_) {}) { |o| log.select { |r| r["offset"] >= o } }
    assert_equal [], resumed.lazy.first(0)
    capped = []
    empty = Loams::Subscription.new(sleeper: ->(s) { capped << s; empty.cancel if capped.size == 6 }) { [] }
    empty.each { flunk "nothing to yield" }
    assert_equal [0.1, 0.2, 0.4, 0.8, 1.0, 1.0], capped, "the backoff stops at 1 s"
  end

  # --- Builders ---------------------------------------------------------------

  def test_ruby_builders_match_corpus
    f = Loams::Filter
    s = Loams::Search
    # The corpus's cases, inline, so they hold where the corpus is absent.
    assert_equal({
      "fields" => [
        { "name" => "title", "source_path" => "title", "kind" => { "text" => { "analyzer" => "standard", "positions" => true } }, "indexed" => true, "fast" => false },
        { "name" => "tags", "source_path" => "tags", "kind" => "keyword", "indexed" => true, "fast" => true },
        { "name" => "n", "source_path" => "n", "kind" => "i64", "indexed" => true, "fast" => true },
      ],
      "vectors" => [{ "name" => "embedding", "dim" => 4, "distance" => "cosine" }],
      "sparse_vectors" => [],
      "dynamic" => "ignore",
      "max_fields" => 1000,
    }, Loams::Schema.new.text("title").keyword("tags").i64("n").vector("embedding", 4).to_h, "schema_notes")
    assert_equal({ "term" => { "field" => "tags", "value" => "policy" } }, f.eq("tags", "policy").to_h, "filter_term")
    assert_equal({ "bool" => { "must" => [
      { "term" => { "field" => "tags", "value" => "policy" } },
      { "bool" => { "should" => [{ "range" => { "field" => "n", "gte" => 2 } }, { "bool" => { "mustNot" => [{ "exists" => { "field" => "archived" } }] } }] } },
    ] } }, f.and(f.eq("tags", "policy"), f.or(f.range("n", gte: 2), ~f.exists("archived"))).to_h, "filter_bool_nested")
    assert_equal({ "range" => { "field" => "n", "gt" => 1 } }, f.range("n", gt: 1).to_h, "filter_range_open")
    assert_equal({ "ids" => [{ "uint" => "1" }, "k-str", { "uuid" => "0190f5c4-6c1e-7b3a-9d2e-4f5a6b7c8d9e" }] },
      f.ids(1, "k-str", { uuid: "0190f5c4-6c1e-7b3a-9d2e-4f5a6b7c8d9e" }).to_h, "filter_ids")
    assert_equal({ "bool" => { "should" => [{ "matchAll" => {} }, { "matchNone" => {} }], "minimumShouldMatch" => "1" } }, f.any_of([f.all, f.none], 1).to_h)
    assert_equal({ "multiMatch" => { "fields" => [{ "field" => "title", "boost" => 2.0 }, { "field" => "body" }], "text" => "q" } }, f.multi_match(["title^2", "body"], "q").to_h)

    text = { "text" => { "query" => { "match" => { "field" => "body", "text" => "refund" } } } }
    assert_equal({
      "collection" => "notes", "retrievers" => [text],
      "filter" => { "term" => { "field" => "tags", "value" => "policy" } },
      "highlight" => { "fields" => [{ "field" => "body" }] },
      "select" => { "source" => { "include" => ["title"] } },
      "limit" => 2, "sort" => [{ "score" => {} }, { "pk" => {} }],
    }, s.on("notes").text("refund", fields: ["body"]).filter(f.eq("tags", "policy")).highlight("body")
      .select(include: ["title"]).limit(2).sort(:score, :pk).to_h, "search_text_filter_highlight_select")
    assert_equal({ "collection" => "notes", "retrievers" => [{ "vector" => { "field" => "embedding", "query" => [1.0, 0.0, 0.0, 0.0], "k" => 3, "params" => { "nprobes" => 8, "refineFactor" => 2 } } }], "limit" => 3 },
      s.on("notes").vector("embedding", [1, 0, 0, 0], k: 3, nprobes: 8, refine_factor: 2).limit(3).to_h, "search_vector_ann_params")
    hybrid = ->(fusion) { s.on("notes").text("refund", fields: ["body"]).vector("embedding", [1, 0, 0, 0], k: 5).fuse(fusion).limit(3).to_h }
    vec = { "vector" => { "field" => "embedding", "query" => [1.0, 0.0, 0.0, 0.0], "k" => 5 } }
    assert_equal({ "collection" => "notes", "retrievers" => [text, vec], "fusion" => { "rrf" => { "k" => 60 } }, "limit" => 3 }, hybrid.call(Loams::Fusion.rrf(60)), "search_hybrid_rrf")
    assert_equal({ "dbsf" => {} }, hybrid.call(Loams::Fusion.dbsf)["fusion"], "search_hybrid_dbsf")
    assert_equal({ "weightedSum" => { "weights" => [0.7, 0.3] } }, hybrid.call(Loams::Fusion.weighted(0.7, 0.3))["fusion"], "search_hybrid_weighted")
    assert_equal [1.25, 2], s.on("notes").text("refund").sort(:score, :pk).after([1.25, 2]).to_h["searchAfter"], "search_after_page"
    assert_equal({ "graph" => "notes_graph", "fromField" => "id", "hops" => 1 }, s.on("notes").expand("notes_graph", from_field: "id", hops: 1).to_h["expandSpec"], "search_expand")
    assert_equal({ "rrf" => {} }, Loams::Fusion.rrf)
    assert_equal({ "atLeast" => "v1:s1/p0@2" }, s.on("n").consistency("v1:s1/p0@2").to_h["consistency"])
    assert_equal({ "freshness" => "FRESHNESS_STRONG" }, s.on("n").consistency(:strong).to_h["consistency"])
    assert_raises(Loams::InvalidArgumentError) { s.on("n").consistency("not-a-token") }
    assert_equal "none", s.on("n").select(no_source: true).to_h.dig("select", "source")

    # build checks a weighted fusion's weights; k goes out only when given.
    err = assert_raises(Loams::InvalidArgumentError) { s.on("n").text("x").vector("v", [1]).fuse(Loams::Fusion.weighted(0.7)).build }
    assert_equal "fusion.weighted_sum.weights", err.metadata["field"]
    wire = Loams::Search.wire(s.on("n").text("x").vector("v", [1], k: 3).limit(25))
    assert_equal [nil, 3], wire["retrievers"].map { |r| r.values.first["k"] }

    # Every builder's JSON decodes as the message it names.
    req = Loams::Descriptors.message("loams.collection.v1.SearchRequest").msgclass
    [hybrid.call(Loams::Fusion.rrf), s.on("notes").expand("g", hops: 1, direction: :both, output: :flat).to_h,
     s.on("notes").sparse("sp", { 3 => 0.5 }).to_h, s.on("notes").select(no_source: true, vectors: ["v"]).to_h,
     s.on("notes").text(f.and(f.eq("a", 1), f.phrase("b", "c d", slop: 1))).group_by("tags", size: 2).min_score(0.5).to_h].each do |json|
      req.decode_json(JSON.generate(json))
    end
    Loams::Descriptors.message("loams.collection.v1.Query").msgclass.decode_json(f.and(f.is_null("a"), f.wildcard("b", "c*"),
      f.query_string("x", fields: ["b"], operator: :and), f.boost(f.all, 2.0), f.constant_score(f.none, 1.0),
      f.multi_match({ "a" => 2.0 }, "q", kind: :best_fields), f.match("a", "b", fuzziness: :auto), f.fuzzy("a", "b", edits: 1)).to_json)

    cases = Loams::Test::BuilderCorpus.cases
    skip "no builder corpus in #{Loams::Test::BuilderCorpus.dir}" if cases.empty?

    cases.each do |c|
      got = Loams::Test::BuilderCorpus.build(c)
      assert Loams::Test::BuilderCorpus.same?(c["message"], c["expect"], got), "#{c["name"]}: #{JSON.generate(got)}"
    end
  end

  # --- The collection handle ---------------------------------------------------

  def test_ruby_collection_handle_delegates
    tp = Loams::Test::StubTransport.new
    answer(tp, RPC[%w[documents write]], {})
    answer(tp, RPC[%w[documents get]], {})
    answer(tp, RPC[%w[documents count]], { "count" => "3" })
    answer(tp, RPC[%w[search query]], {})
    answer(tp, RPC[%w[vector search]], {})
    answer(tp, RPC[%w[collections get]], {})
    answer(tp, RPC[%w[collections drop]], { "dropped" => true })
    client = stub_client(tp, namespace: "app")
    notes = client.collection("notes", namespace: "other")
    notes.upsert([{ id: "a", source: { x: 1 } }])
    notes.get(["a"])
    assert_equal 3, notes.count.count
    notes.search(Loams::Search.on("ignored").text("x", fields: ["body"]))
    notes.vector_search([1, 0], field: "embedding")
    notes.info
    assert notes.drop.dropped
    paths = tp.requests.map(&:path)
    assert_equal %w[WriteDocuments GetDocuments CountDocuments Search Search GetCollection DropCollection], paths.map { |p| p.split("/").last }
    tp.requests.each do |r|
      msg = Loams::Descriptors.binding_for_rpc(r.path.delete_prefix("/")).request.msgclass.decode(r.body)
      assert_equal "other", msg.namespace
      assert_equal "notes", msg.collection
    end
  end

  def test_ruby_search_query_all_requires_tiebreak
    tp = Loams::Test::StubTransport.new
    client = stub_client(tp, namespace: "app")
    err = assert_raises(Loams::InvalidArgumentError) { client.search.query_all(Loams::Search.on("notes").text("x").sort(:score)).to_a }
    assert_equal :search_after_needs_tiebreak, err.reason
    assert_empty tp.requests

    hit = ->(pk, sv) { { "pk" => { "uint" => pk.to_s }, "sortValues" => sv } }
    answer(tp, RPC[%w[search query]], { "hits" => [hit.call(1, [0.9, 1]), hit.call(2, [0.8, 2])] })
    answer(tp, RPC[%w[search query]], { "hits" => [hit.call(3, [0.7, 3])] })
    hits = client.search.query_all(Loams::Search.on("notes").text("x", fields: ["b"]).sort(:score, :pk).limit(2)).to_a
    assert_equal [1, 2, 3], hits.map { |h| Loams::Values.from_document_id(h.pk) }
    second = sent(tp, RPC[%w[search query]], 1)
    assert_equal [0.8, 2.0], second.search_after.map { |v| Loams.to_h(v) }, "the last hit's sort_values, as answered"
  end

  # --- The facade index ----------------------------------------------------------

  def test_ruby_sql_and_streams_send_the_recorded_bodies
    tp = Loams::Test::StubTransport.new
    answer(tp, RPC[%w[sql query]], { "columns" => [{ "name" => "tags" }, { "name" => "c" }], "rows" => [["policy", 2]] })
    answer(tp, RPC[%w[sql query]], {})
    answer(tp, RPC[%w[streams create]], {})
    answer(tp, RPC[%w[streams produce]], {})
    answer(tp, RPC[%w[streams fetch]], {})
    answer(tp, RPC[%w[streams fetch]], {})
    answer(tp, RPC[%w[streams get]], {})
    client = stub_client(tp, namespace: "app")
    body = ->(i, path) { JSON.parse(binding(path).request.msgclass.encode_json(sent(tp, path, i))) }

    res = client.sql.query("SELECT tags, count(*) AS c FROM notes WHERE n > $1 GROUP BY tags ORDER BY tags", params: [1])
    assert_equal({ "namespace" => "app", "sql" => "SELECT tags, count(*) AS c FROM notes WHERE n > $1 GROUP BY tags ORDER BY tags", "params" => [1.0] }, body.call(0, RPC[%w[sql query]]))
    assert_equal [{ "tags" => "policy", "c" => 2.0 }], Loams::App::Sql.records(res)
    client.sql.query("SELECT title, n FROM notes ORDER BY n", max_rows: 2)
    assert_equal({ "namespace" => "app", "sql" => "SELECT title, n FROM notes ORDER BY n", "maxRows" => 2 }, body.call(1, RPC[%w[sql query]]))
    client.streams.create("note-events")
    assert_equal({ "namespace" => "app", "name" => "note-events", "partitions" => 1 }, body.call(2, RPC[%w[streams create]]))
    client.streams.produce("note-events", [{ key: "1", value: { "note" => 1, "op" => "created" } }, { key: "5", value: { "note" => 5, "op" => "deleted" } }],
      producer_id: "app-producer-1", producer_seq: 1)
    assert_equal({ "namespace" => "app", "stream" => "note-events", "records" => [
      { "key" => "MQ==", "value" => "eyJub3RlIjoxLCJvcCI6ImNyZWF0ZWQifQ==" },
      { "key" => "NQ==", "value" => "eyJub3RlIjo1LCJvcCI6ImRlbGV0ZWQifQ==" },
    ], "producerId" => "app-producer-1", "producerSeq" => "1" }, body.call(3, RPC[%w[streams produce]]))
    client.streams.fetch("note-events", max_records: 10)
    assert_equal({ "namespace" => "app", "stream" => "note-events", "maxRecords" => 10 }, body.call(4, RPC[%w[streams fetch]]), "a zero partition and offset are omitted")
    client.streams.fetch("note-events", offset: 2, max_records: 10)
    assert_equal({ "namespace" => "app", "stream" => "note-events", "offset" => "2", "maxRecords" => 10 }, body.call(5, RPC[%w[streams fetch]]))
    client.streams.get("note-events")
    assert_equal({ "namespace" => "app", "stream" => "note-events" }, body.call(6, RPC[%w[streams get]]))
  end

  def test_ruby_streams_module_subscribe
    tp = Loams::Test::StubTransport.new
    answer(tp, RPC[%w[streams fetch]], { "records" => [{ "offset" => "1", "value" => ["a"].pack("m0") }, { "offset" => "2", "value" => ["b"].pack("m0") }] })
    answer(tp, RPC[%w[streams fetch]], { "records" => [{ "offset" => "3", "value" => ["c"].pack("m0") }] })
    sub = stub_client(tp, namespace: "app").streams.subscribe("events", from_offset: 1)
    got = []
    sub.each do |record|
      got << record.value
      sub.cancel if got.size == 3
    end
    assert_equal %w[a b c], got
    assert_equal 4, sub.offset
    assert_equal 1, sent(tp, RPC[%w[streams fetch]], 0).offset
    assert_equal 3, sent(tp, RPC[%w[streams fetch]], 1).offset, "the next fetch starts after the last record"
  end

  def test_ruby_facade_matches_index
    path = File.expand_path("../../fixtures/facade-index.json", __dir__)
    app_modules = Loams::App::CALLS.keys.map(&:first).uniq
    # Without the index (P0a), check what can be checked: every app call's RPC
    # is in the descriptor set and its module answers the call.
    client = Loams::Client.new("http://127.0.0.1:1", transport: Loams::Test::StubTransport.new)
    Loams::App::CALLS.each do |(mod, call), rpc|
      refute_nil binding(rpc), "#{rpc} is in the descriptor set"
      assert client.public_send(mod).respond_to?(call), "#{mod}.#{call}"
    end
    return unless File.exist?(path)

    index = JSON.parse(File.read(path))
    calls = index.fetch("calls").select { |c| app_modules.include?(c["module"]) }
    calls.each do |c|
      key = [c["module"], Loams::Descriptors.snake(c["call"])]
      assert_equal c["rpc"], Loams::App::CALLS[key], "#{key.join(".")} names the index's RPC"
      b = binding(c["rpc"])
      klass = if b.takes_idempotency_key? && b.retry != :safe then "keyed" elsif b.retry == :safe then "safe" else "never" end
      assert_equal c["retry"], klass, "#{key.join(".")} retry class" if c["retry"]
    end
  end

  # Part D: the README's example compiles; it needs a server to run.
  def test_ruby_example_notes_compiles
    Dir[File.expand_path("../examples/**/*.rb", __dir__)].each do |file|
      RubyVM::InstructionSequence.compile_file(file)
    end
  end
end

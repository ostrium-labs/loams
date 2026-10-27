# frozen_string_literal: true

require "json"
require "loams/errors"
require "loams/consistency"

module Loams
  # The typed builders of the app-building API (SDKP §2.4 and §2.6): `Schema`,
  # `Filter` and `Search`. Each is a pure function onto one request: `to_h`
  # returns the request's **proto3 JSON** (string keys, `lowerCamelCase` field
  # names, enum names as strings), which is what the builder corpus pins and
  # what `decode_json` turns into the message the invoker sends.
  #
  # Nothing here talks to a server, and nothing validates what the server
  # validates: a `weighted` fusion with the wrong number of weights is sent and
  # refused by the server with its field violation.
  module Builders
    # Accepts a builder, a Hash or nil, and answers proto3 JSON or nil.
    def self.json(value)
      case value
      when nil then nil
      when Hash then value
      else value.respond_to?(:to_h) ? value.to_h : value
      end
    end

    # Removes nil values, so an unset optional field is absent.
    def self.compact(hash)
      hash.reject { |_, v| v.nil? }
    end
  end

  # The `CreateCollectionRequest.schema` Struct, in the shape the server's own
  # schema parser reads (`kb_schema()` in `connect_collections.rs`).
  #
  #   Loams::Schema.new
  #     .text("body").keyword("tags").i64("n")
  #     .vector("embedding", 4)
  class Schema
    def initialize
      @fields = []
      @vectors = []
      @sparse = []
      @dynamic = "ignore"
      @max_fields = 1000
    end

    # A full-text field. `positions` is what phrase queries need.
    def text(name, analyzer: "standard", positions: true, indexed: true, fast: false, source_path: nil)
      field(name, { "text" => { "analyzer" => analyzer, "positions" => positions } }, indexed, fast, source_path)
    end

    # An exact-match field: terms, prefixes, facets.
    def keyword(name, indexed: true, fast: true, source_path: nil)
      field(name, "keyword", indexed, fast, source_path)
    end

    def i64(name, indexed: true, fast: true, source_path: nil)
      field(name, "i64", indexed, fast, source_path)
    end

    def f64(name, indexed: true, fast: true, source_path: nil)
      field(name, "f64", indexed, fast, source_path)
    end

    def bool(name, indexed: true, fast: true, source_path: nil)
      field(name, "bool", indexed, fast, source_path)
    end

    # A date-time field (the schema's `date` kind).
    def timestamp(name, indexed: true, fast: true, source_path: nil)
      field(name, "date", indexed, fast, source_path)
    end

    def uuid(name, indexed: true, fast: true, source_path: nil)
      field(name, "uuid", indexed, fast, source_path)
    end

    # A dense vector of +dim+ floats.
    def vector(name, dim, distance: "cosine")
      @vectors << { "name" => name.to_s, "dim" => Integer(dim), "distance" => distance.to_s }
      self
    end

    def sparse_vector(name, modifier: nil)
      @sparse << Builders.compact({ "name" => name.to_s, "modifier" => modifier&.to_s })
      self
    end

    # What a document field the schema does not name does: `"ignore"` (the
    # builder's default), `"strict"` (the server's) or `"map"`.
    def dynamic(mode)
      mode = mode.to_s
      raise ArgumentError, "dynamic must be ignore, strict or map" unless %w[ignore strict map].include?(mode)

      @dynamic = mode
      self
    end

    def max_fields(count)
      @max_fields = Integer(count)
      self
    end

    def to_h
      {
        "fields" => @fields.map(&:dup),
        "vectors" => @vectors.map(&:dup),
        "sparse_vectors" => @sparse.map(&:dup),
        "dynamic" => @dynamic,
        "max_fields" => @max_fields,
      }
    end

    def to_json(*args)
      to_h.to_json(*args)
    end

    private

    def field(name, kind, indexed, fast, source_path)
      @fields << {
        "name" => name.to_s,
        "source_path" => (source_path || name).to_s,
        "kind" => kind,
        "indexed" => indexed,
        "fast" => fast,
      }
      self
    end
  end

  # One `loams.collection.v1.Query`: the filter of a search, the query of a
  # text retriever, and the typed `where` of the document reads.
  #
  #   Loams::Filter.and(Loams::Filter.eq("tags", "policy"),
  #                     Loams::Filter.range("n", gte: 2))
  class Filter
    attr_reader :to_h

    def initialize(json)
      @to_h = json.freeze
    end

    def to_json(*args)
      to_h.to_json(*args)
    end

    def ==(other)
      other.is_a?(Filter) && other.to_h == to_h
    end

    # `self & other`, `self | other` and `~self`: `and`, `or` and `not`.
    def &(other)
      Filter.and(self, other)
    end

    def |(other)
      Filter.or(self, other)
    end

    def ~
      Filter.not(self)
    end

    class << self
      def all
        new({ "matchAll" => {} })
      end

      def none
        new({ "matchNone" => {} })
      end

      # `field` equals +value+ (a term query).
      def eq(field, value)
        new({ "term" => { "field" => field.to_s, "value" => value } })
      end
      alias term eq

      # `field` is one of +values+ (a terms query).
      def in(field, values)
        new({ "terms" => { "field" => field.to_s, "values" => Array(values) } })
      end
      alias terms in

      def range(field, gt: nil, gte: nil, lt: nil, lte: nil)
        new({ "range" => Builders.compact({ "field" => field.to_s, "gt" => gt, "gte" => gte, "lt" => lt, "lte" => lte }) })
      end

      def exists(field)
        new({ "exists" => { "field" => field.to_s } })
      end

      def is_null(field)
        new({ "isNull" => { "field" => field.to_s } })
      end

      def is_empty(field)
        new({ "isEmpty" => { "field" => field.to_s } })
      end

      def prefix(field, value)
        new({ "prefix" => { "field" => field.to_s, "value" => value.to_s } })
      end

      def wildcard(field, pattern)
        new({ "wildcard" => { "field" => field.to_s, "pattern" => pattern.to_s } })
      end

      # +edits+ is a number of edits or `:auto`; nil leaves the server's default.
      def fuzzy(field, value, edits: nil)
        new({ "fuzzy" => Builders.compact({ "field" => field.to_s, "value" => value.to_s, "fuzziness" => fuzziness(edits) }) })
      end

      # Full-text match; +operator+ is `"or"` (default) or `"and"`.
      def match(field, text, operator: nil, minimum_should_match: nil, fuzziness: nil, analyzer: nil)
        new({ "match" => Builders.compact({
          "field" => field.to_s, "text" => text.to_s,
          "operator" => operator&.to_s, "minimumShouldMatch" => minimum_should_match&.to_s,
          "fuzziness" => fuzziness(fuzziness), "analyzer" => analyzer&.to_s,
        }) })
      end

      def phrase(field, text, slop: nil)
        new({ "matchPhrase" => Builders.compact({ "field" => field.to_s, "text" => text.to_s, "slop" => slop }) })
      end

      KINDS = %w[best_fields most_fields cross_fields phrase phrase_prefix].freeze

      # One text over several fields: names, `"name^boost"`, or `{name => boost}`.
      # +kind+ is one of `KINDS`; +operator+ `"or"` or `"and"`.
      def multi_match(fields, text, kind: nil, operator: nil, tie_breaker: nil)
        list = if fields.is_a?(Hash)
          fields.map { |f, b| { "field" => f.to_s, "boost" => b } }
        else
          Array(fields).map { |f| boosted(f.to_s) }
        end
        new({ "multiMatch" => Builders.compact({
          "fields" => list, "text" => text.to_s,
          "kind" => kind && "MULTI_MATCH_KIND_#{kind.to_s.upcase}",
          "operator" => operator && bool_operator(operator), "tieBreaker" => tie_breaker,
        }) })
      end

      def query_string(query, fields: nil, operator: nil)
        new({ "queryString" => Builders.compact({
          "query" => query.to_s,
          "defaultFields" => fields.nil? || fields.empty? ? nil : fields.map(&:to_s),
          "defaultOperator" => operator && bool_operator(operator),
        }) })
      end

      # Documents by primary key: integers, strings or `{"uuid" => …}`.
      def ids(*ids)
        new({ "ids" => ids.flatten.map { |id| id_value(id) } })
      end

      # A primary key as `Query.ids` spells it: a number is `{"uint" => "…"}`,
      # a string stays a string, and `{uuid: …}` is `{"uuid" => …}`.
      def id_value(id)
        case id
        when Integer
          raise ArgumentError, "a numeric document id is non-negative" if id.negative?

          { "uint" => id.to_s }
        when String, Symbol then id.to_s
        when Hash then id.transform_keys(&:to_s).transform_values(&:to_s)
        else raise ArgumentError, "a document id is an Integer, a String or {uuid: …}"
        end
      end

      # Every clause must match (`bool.must`).
      def and(*clauses)
        new({ "bool" => { "must" => clauses.flatten.map { |c| Builders.json(c) } } })
      end

      # Any clause matches (`bool.should`).
      def or(*clauses)
        new({ "bool" => { "should" => clauses.flatten.map { |c| Builders.json(c) } } })
      end

      # At least +minimum_should_match+ of +clauses+ match.
      def any_of(clauses, minimum_should_match)
        new({ "bool" => {
          "should" => Array(clauses).map { |c| Builders.json(c) },
          "minimumShouldMatch" => minimum_should_match.to_s,
        } })
      end

      # No clause matches (`bool.must_not`).
      def not(*clauses)
        new({ "bool" => { "mustNot" => clauses.flatten.map { |c| Builders.json(c) } } })
      end

      # Every clause matches, without scoring (`bool.filter`).
      def filter(*clauses)
        new({ "bool" => { "filter" => clauses.flatten.map { |c| Builders.json(c) } } })
      end

      def boost(clause, boost)
        new({ "boost" => { "query" => Builders.json(clause), "boost" => boost } })
      end

      def constant_score(clause, score)
        new({ "constantScore" => { "query" => Builders.json(clause), "score" => score } })
      end

      private

      def boosted(spec)
        at = spec.rindex("^")
        if at&.positive?
          boost = Float(spec[(at + 1)..], exception: false)
          return { "field" => spec[0...at], "boost" => boost } if boost&.finite?
        end
        { "field" => spec }
      end

      def bool_operator(operator)
        operator.to_s == "and" ? "BOOL_OPERATOR_AND" : "BOOL_OPERATOR_OR"
      end

      def fuzziness(edits)
        case edits
        when nil then nil
        when :auto, "auto" then { "auto" => {} }
        else { "edits" => Integer(edits) }
        end
      end
    end
  end

  # How a hybrid search fuses its retrievers' rankings.
  module Fusion
    # Reciprocal rank fusion; nil leaves `k` at the server's default (60).
    def self.rrf(k = nil)
      { "rrf" => k.nil? ? {} : { "k" => Integer(k) } }
    end

    # Distribution-based score fusion.
    def self.dbsf
      { "dbsf" => {} }
    end

    # A weighted sum, one weight per retriever.
    def self.weighted(*weights)
      { "weightedSum" => { "weights" => weights.flatten } }
    end
  end

  # One sort key.
  module Sort
    ORDERS = { asc: "SORT_ORDER_ASC", desc: "SORT_ORDER_DESC" }.freeze
    MISSING = { first: "MISSING_ORDER_FIRST", last: "MISSING_ORDER_LAST" }.freeze

    def self.score(order = nil)
      { "score" => Builders.compact({ "order" => order(order) }) }
    end

    # The primary key: the tiebreaker `query_all` requires.
    def self.pk(order = nil)
      { "pk" => Builders.compact({ "order" => order(order) }) }
    end

    def self.field(name, order = nil, missing: nil)
      { "field" => Builders.compact({ "field" => name.to_s, "order" => order(order), "missing" => missing && MISSING.fetch(missing.to_sym) }) }
    end

    def self.order(order)
      order && ORDERS.fetch(order.to_sym)
    end
    private_class_method :order
  end

  # A `SearchRequest`, verb by verb:
  #
  #   Loams::Search.on("notes")
  #     .text("refund", fields: ["body"])
  #     .vector("embedding", [1, 0, 0, 0])
  #     .fuse(Loams::Fusion.rrf(60))
  #     .filter(Loams::Filter.eq("tags", "policy"))
  #     .limit(2)
  #
  # `to_h` is the request exactly as built (what the builder corpus pins);
  # `build` checks it too, and `search.query` sends `build`.
  class Search
    # The page size `query_all` uses when the request sets no `limit`.
    DEFAULT_K = 10

    def self.on(collection, namespace: nil)
      new(collection, namespace: namespace)
    end

    def initialize(collection = nil, namespace: nil)
      @req = {}
      @req["namespace"] = namespace.to_s unless namespace.nil?
      @req["collection"] = collection.to_s unless collection.nil?
    end

    def initialize_copy(source)
      super
      @req = Marshal.load(Marshal.dump(@req))
    end

    def collection(name)
      @req["collection"] = name.to_s
      self
    end

    def namespace(name)
      @req["namespace"] = name.to_s
      self
    end

    # The namespace this request names, or nil.
    def namespace_name
      @req["namespace"]
    end

    def collection_name
      @req["collection"]
    end

    # A text retriever. +query+ is a `Filter` (any query), or a string: a
    # match on one field, a multi-match on several, or a query string when
    # no field is given. `field:` is the one-field spelling of `fields:`.
    def text(query, fields: nil, field: nil, k: nil)
      fields = [field] if fields.nil? && !field.nil?
      q = case query
      when Filter, Hash then Builders.json(query)
      else
        text = query.to_s
        if fields.nil? || fields.empty? then Filter.query_string(text).to_h
        elsif fields.size == 1 then Filter.match(fields.first, text).to_h
        else Filter.multi_match(fields, text).to_h
        end
      end
      retriever(:text, Builders.compact({ "query" => q, "k" => k }))
    end

    # A dense-vector (kNN) retriever. The ANN params are the server's
    # `AnnParams`; nil leaves each at its default.
    def vector(field, values, k: nil, exact: nil, nprobes: nil, refine_factor: nil, ef: nil,
      oversampling: nil, distance: nil, filter: nil)
      params = Builders.compact({
        "exact" => exact, "nprobes" => nprobes, "refineFactor" => refine_factor, "ef" => ef,
        "oversampling" => oversampling, "distance" => distance&.to_s,
      })
      body = { "field" => field.to_s, "query" => values.map(&:to_f) }
      body["k"] = k unless k.nil?
      body["params"] = params unless params.empty?
      body["filter"] = Builders.json(filter) unless filter.nil?
      retriever(:vector, body)
    end

    # A sparse-vector retriever; +terms+ is `{index => weight}` or
    # `{indices: […], values: […]}`.
    def sparse(field, terms, k: nil, filter: nil)
      terms = terms.transform_keys(&:to_s)
      query = if terms.key?("indices")
        { "indices" => terms["indices"].map { |i| Integer(i) }, "values" => terms["values"].map(&:to_f) }
      else
        { "indices" => terms.keys.map { |i| Integer(i) }, "values" => terms.values.map(&:to_f) }
      end
      retriever(:sparse, Builders.compact({ "field" => field.to_s, "query" => query, "k" => k, "filter" => filter && Builders.json(filter) }))
    end

    def fuse(fusion)
      @req["fusion"] = Builders.json(fusion)
      self
    end

    def filter(filter)
      @req["filter"] = Builders.json(filter)
      self
    end

    # Sort keys, in order: `Sort.score`, `Sort.pk`, `Sort.field`, or the
    # symbols `:score` and `:pk`.
    def sort(*keys)
      @req["sort"] = keys.flatten.map do |key|
        case key
        when :score then Sort.score
        when :pk then Sort.pk
        else Builders.json(key)
        end
      end
      self
    end

    def limit(count)
      @req["limit"] = Integer(count)
      self
    end

    def offset(count)
      @req["offset"] = Integer(count)
      self
    end

    # Continue after a hit: its `sort_values`.
    def after(sort_values)
      @req["searchAfter"] = Array(sort_values)
      self
    end

    def min_score(score)
      @req["scoreThreshold"] = score
      self
    end

    # The projection: which `_source` paths (or `no_source: true`), vectors
    # and typed fields to return.
    def select(include: nil, exclude: nil, no_source: false, vectors: nil, fields: nil)
      source = Builders.compact({ "include" => include&.map(&:to_s), "exclude" => exclude&.map(&:to_s) })
      @req["select"] = Builders.compact({
        "source" => no_source ? "none" : (source.empty? ? nil : source),
        "vectors" => vectors&.map(&:to_s),
        "fields" => fields&.map(&:to_s),
      })
      self
    end

    def highlight(field, pre: nil, post: nil, fragment_size: nil, fragments: nil)
      existing = @req.dig("highlight", "fields") || []
      @req["highlight"] = { "fields" => existing + [Builders.compact({
        "field" => field.to_s, "preTag" => pre, "postTag" => post,
        "fragmentSize" => fragment_size, "numberOfFragments" => fragments,
      })] }
      self
    end

    def group_by(field, size: nil, limit: nil)
      @req["groupBy"] = Builders.compact({ "field" => field.to_s, "groupSize" => size, "limit" => limit })
      self
    end

    # Graph expansion of the hits (`expand_spec`); +direction+ is `:out`,
    # `:in` or `:both`, +output+ `:nested` or `:flat`.
    def expand(graph, from_field: nil, seed_label: nil, seeds: nil, hops: nil, direction: nil,
      edge_types: nil, limit_per_seed: nil, limit: nil, output: nil)
      @req["expandSpec"] = Builders.compact({
        "graph" => graph.to_s, "fromField" => from_field&.to_s, "seedLabel" => seed_label&.to_s,
        "seeds" => seeds, "hops" => hops, "limitPerSeed" => limit_per_seed, "limit" => limit,
        "direction" => direction && "EXPAND_DIRECTION_#{direction.to_s.upcase}",
        "edgeTypes" => edge_types&.map(&:to_s),
        "output" => output && "EXPAND_OUTPUT_#{output.to_s.upcase}",
      })
      self
    end

    # `:strong`, `:eventual`, or a token (a String, checked here, or a
    # `ConsistencyToken`) as `at_least`.
    def consistency(level)
      @req["consistency"] = Search.consistency_json(level)
      self
    end

    def self.consistency_json(level)
      case level
      when nil then nil
      when :strong, "strong" then { "freshness" => "FRESHNESS_STRONG" }
      when :eventual, "eventual" then { "freshness" => "FRESHNESS_EVENTUAL" }
      when Hash then level
      when ConsistencyToken then { "atLeast" => level.to_s }
      else { "atLeast" => ConsistencyToken.parse(level.to_s).to_s }
      end
    end

    # Whether the request's last sort key is the primary key, the tiebreaker
    # `query_all` needs.
    def tiebreak?
      Search.tiebreak?(@req)
    end

    def self.tiebreak?(request)
      last = Array(request["sort"]).last
      last.is_a?(Hash) && last.key?("pk")
    end

    def to_h
      Marshal.load(Marshal.dump(@req))
    end

    def to_json(*args)
      to_h.to_json(*args)
    end

    # `to_h`, checked: a weighted fusion needs one weight per retriever.
    def build
      Search.check(to_h)
    end

    def self.check(request)
      weights = request.dig("fusion", "weightedSum", "weights")
      count = Array(request["retrievers"]).size
      if weights && weights.size != count
        raise InvalidArgumentError.new(
          "a weighted fusion needs one weight per retriever: #{weights.size} weights for #{count} retrievers",
          code: :invalid_argument,
          metadata: { "field" => "fusion.weighted_sum.weights" },
        )
      end
      request
    end

    # What the app modules send: `build`. A retriever's `k` goes out only when
    # the caller gave one; the engine fills `max(10, offset + limit)` itself
    # (SDKP-15, SDKP-18).
    def self.wire(request)
      check(request.is_a?(Search) ? request.to_h : Marshal.load(Marshal.dump(request)))
    end

    private

    def retriever(kind, body)
      @req["retrievers"] = (@req["retrievers"] || []) + [{ kind.to_s => body }]
      self
    end
  end
end

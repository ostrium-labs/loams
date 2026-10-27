# frozen_string_literal: true

require "json"
require "loams/descriptors"
require "loams/builders"
require "loams/batch"
require "loams/consistency"
require "loams/pagination"
require "loams/subscribe"
require "loams/values"

module Loams
  # The app-building modules (SDKP §2.2–§2.5): `namespaces`, `collections`,
  # `documents`, `search`, `vector` and `graph`.
  #
  # Each call names its RPC and builds the request's proto3 JSON; the binding
  # (retry class, idempotency-key field, request and response types) is read
  # from the committed descriptor set, never typed here. Requests go through
  # the client's invoker, so retries, keys, token refresh, timeouts and errors
  # are R1–R12 as for every other module. Answers are the response messages;
  # `Loams.to_h` turns one into plain Ruby.
  module App
    # The RPCs behind every app call, `[module, call] => rpc`. Checked against
    # `sdks/fixtures/facade-index.json` by `ruby_facade_matches_index`.
    CALLS = {
      %w[namespaces create] => "loams.collection.v1.NamespaceService/CreateNamespace",
      %w[namespaces list] => "loams.collection.v1.NamespaceService/ListNamespaces",
      %w[collections create] => "loams.collection.v1.CollectionService/CreateCollection",
      %w[collections list] => "loams.collection.v1.CollectionService/ListCollections",
      %w[collections get] => "loams.collection.v1.CollectionService/GetCollection",
      %w[collections drop] => "loams.collection.v1.CollectionService/DropCollection",
      %w[collections add_fields] => "loams.collection.v1.CollectionService/AddFields",
      %w[collections list_versions] => "loams.collection.v1.CollectionService/ListVersions",
      %w[collections update_aliases] => "loams.collection.v1.CollectionService/UpdateAliases",
      %w[collections scan] => "loams.collection.v1.CollectionService/Scan",
      %w[collections set_hot] => "loams.collection.v1.CollectionService/SetHot",
      %w[collections warm] => "loams.collection.v1.CollectionService/WarmCollection",
      %w[documents write] => "loams.collection.v1.DocumentService/WriteDocuments",
      %w[documents get] => "loams.collection.v1.DocumentService/GetDocuments",
      %w[documents scroll] => "loams.collection.v1.DocumentService/ScrollDocuments",
      %w[documents count] => "loams.collection.v1.DocumentService/CountDocuments",
      %w[documents delete_by_filter] => "loams.collection.v1.DocumentService/DeleteByFilter",
      %w[documents patch_by_filter] => "loams.collection.v1.DocumentService/PatchByFilter",
      %w[search query] => "loams.collection.v1.QueryService/Search",
      %w[vector search] => "loams.collection.v1.QueryService/Search",
      %w[graph create] => "loams.graph.v1.GraphAdminService/CreateGraph",
      %w[graph get] => "loams.graph.v1.GraphAdminService/GetGraph",
      %w[graph list] => "loams.graph.v1.GraphAdminService/ListGraphs",
      %w[graph update] => "loams.graph.v1.GraphAdminService/UpdateGraph",
      %w[graph delete] => "loams.graph.v1.GraphAdminService/DeleteGraph",
      %w[graph schema] => "loams.graph.v1.GraphAdminService/GetSchema",
      %w[graph query] => "loams.graph.v1.GraphService/Execute",
      %w[graph query_batch] => "loams.graph.v1.GraphService/ExecuteBatch",
      %w[graph query_stream] => "loams.graph.v1.GraphService/ExecuteStream",
      %w[graph explain] => "loams.graph.v1.GraphService/Explain",
      %w[sql query] => "loams.sql.v1.SqlService/Query",
      %w[sql query_arrow] => "loams.sql.v1.SqlService/QueryArrow",
      %w[streams create] => "loams.stream.v1.StreamService/CreateStream",
      %w[streams get] => "loams.stream.v1.StreamService/DescribeStream",
      %w[streams list] => "loams.stream.v1.StreamService/ListStreams",
      %w[streams produce] => "loams.stream.v1.StreamService/Produce",
      %w[streams fetch] => "loams.stream.v1.StreamService/Fetch",
      %w[streams produce_events] => "loams.stream.v1.StreamService/ProduceCloudEvents",
      %w[streams fetch_events] => "loams.stream.v1.StreamService/FetchCloudEvents",
    }.freeze

    # The packages whose reads take the session's token (SDKP-17).
    SESSION_SCOPES = %w[loams.collection.v1].freeze

    # What every module shares: the call path from proto3 JSON to an answer.
    class Base
      def initialize(client)
        @client = client
      end

      private

      # Sends +request+ (proto3 JSON, or a message) to +rpc+.
      #
      # R11: a request with a `namespace` field gets the client's default
      # namespace when it leaves it empty, and with neither it is refused
      # here, before any network call. R4: with `session_consistency`, a read
      # that leaves `consistency` unset reads at least the session's token, and
      # every token an answer carries is merged into the session.
      def rpc(path, request, options = {})
        binding = Descriptors.binding_for_rpc(path)
        raise UnimplementedError.new("this SDK's descriptor set has no #{path}", code: :unimplemented) if binding.nil?

        message = to_message(binding, request)
        fill_namespace(binding, message, path)
        # SQL reads collections, so it reads (and adds to) the collection token.
        scope = binding.package_name == "loams.sql.v1" ? "loams.collection.v1" : binding.package_name
        # SDKP-17: only collection (and SQL) reads take the session's token; a
        # graph's tokens name one graph's stream.
        @client.session&.apply(scope, message) if SESSION_SCOPES.include?(scope)
        call_options = options.dup
        if binding.server_streaming?
          return @client.invoker.server_stream(binding, message, call_options)
        end

        answer = @client.invoker.unary(binding, message, call_options)
        @client.session&.observe_message(scope, answer)
        answer
      end

      def to_message(binding, request)
        return request if request.is_a?(binding.request.msgclass)

        binding.request.msgclass.decode_json(JSON.generate(request || {}))
      rescue Google::Protobuf::ParseError => e
        raise InvalidArgumentError.new("#{binding.rpc}: #{e.message}", code: :invalid_argument, rpc: binding.rpc)
      end

      def fill_namespace(binding, message, path)
        field = binding.request.lookup("namespace")
        return if field.nil? || field.type != :string

        current = message["namespace"]
        return unless current.nil? || current.empty?

        default = @client.namespace
        if default.nil? || default.empty?
          raise InvalidArgumentError.new(
            "#{path} needs a namespace: pass namespace: or give the client one",
            code: :invalid_argument,
            reason: :namespace_required,
            rpc: path,
          )
        end
        message["namespace"] = default
      end

      # `{namespace: ns}` when +ns+ is given, so the client default applies
      # otherwise.
      def ns(namespace)
        namespace.nil? ? {} : { "namespace" => namespace.to_s }
      end

      def call_opts(kw)
        kw.slice(:timeout, :headers, :idempotency_key, :max_retries)
      end

      # Every item of a paged list, following `next_page_token`.
      def paged(path, request, items, options)
        binding = Descriptors.binding_for_rpc(path)
        PageIterator.new(
          fetch: proc { |token| rpc(path, request.merge("pageToken" => token.to_s), options) },
          items_field: binding.response.lookup(items),
          next_page_token_field: binding.response.lookup("next_page_token"),
        ).each
      end
    end

    # Any app call by its facade name, over its request's proto3 JSON: what
    # `Client#call` goes through.
    class Raw < Base
      def call(name, request, **opts)
        mod, call = name.split(".", 2)
        path = CALLS[[mod, call]] or raise ArgumentError, "no app call #{name}"
        rpc(path, request, call_opts(opts))
      end
    end

    # `namespaces`: the container every collection and stream lives in.
    class Namespaces < Base
      def create(namespace, **opts)
        rpc(CALLS[%w[namespaces create]], { "namespace" => namespace.to_s }, call_opts(opts))
      end

      def list(page_size: nil, page_token: nil, **opts)
        rpc(CALLS[%w[namespaces list]], Builders.compact({ "pageSize" => page_size, "pageToken" => page_token }), call_opts(opts))
      end

      # Every namespace, across pages, as an Enumerator.
      def list_all(page_size: nil, **opts)
        paged(CALLS[%w[namespaces list]], Builders.compact({ "pageSize" => page_size }), "namespaces", call_opts(opts))
      end
    end

    # `collections`: schema, aliases, versions and the hot tier.
    class Collections < Base
      # +schema+ is a `Schema` or its Hash.
      def create(name, schema:, partitions: nil, namespace: nil, **opts)
        rpc(CALLS[%w[collections create]], ns(namespace).merge(Builders.compact({
          "name" => name.to_s, "schema" => Builders.json(schema), "partitions" => partitions,
        })), call_opts(opts))
      end

      def get(name, namespace: nil, **opts)
        rpc(CALLS[%w[collections get]], ns(namespace).merge("collection" => name.to_s), call_opts(opts))
      end

      def list(page_size: nil, page_token: nil, namespace: nil, **opts)
        rpc(CALLS[%w[collections list]], ns(namespace).merge(Builders.compact({ "pageSize" => page_size, "pageToken" => page_token })), call_opts(opts))
      end

      def list_all(page_size: nil, namespace: nil, **opts)
        paged(CALLS[%w[collections list]], ns(namespace).merge(Builders.compact({ "pageSize" => page_size })), "collections", call_opts(opts))
      end

      def drop(name, namespace: nil, **opts)
        rpc(CALLS[%w[collections drop]], ns(namespace).merge("collection" => name.to_s), call_opts(opts))
      end

      # Additive schema change: +fields+ and +vectors+ are the schema
      # builder's field and vector Hashes (`Schema#to_h["fields"]`).
      def add_fields(name, fields: [], vectors: [], annotations: nil, namespace: nil, **opts)
        fields = Builders.json(fields)
        fields = fields["fields"] if fields.is_a?(Hash)
        rpc(CALLS[%w[collections add_fields]], ns(namespace).merge(Builders.compact({
          "collection" => name.to_s, "fields" => fields, "vectors" => vectors, "annotations" => annotations,
        })), call_opts(opts))
      end

      def list_versions(name, page_size: nil, page_token: nil, namespace: nil, **opts)
        rpc(CALLS[%w[collections list_versions]], ns(namespace).merge(Builders.compact({
          "collection" => name.to_s, "pageSize" => page_size, "pageToken" => page_token,
        })), call_opts(opts))
      end

      # +actions+: `[{create: {alias:, collection:}}, {delete: {alias:}}]`.
      def update_aliases(actions, namespace: nil, **opts)
        rpc(CALLS[%w[collections update_aliases]], ns(namespace).merge("actions" => actions), call_opts(opts))
      end

      def scan(name, at: nil, namespace: nil, **opts)
        rpc(CALLS[%w[collections scan]], ns(namespace).merge(Builders.compact({ "collection" => name.to_s, "at" => at })), call_opts(opts))
      end

      def set_hot(name, vectors: false, text: false, fragments: false, namespace: nil, **opts)
        rpc(CALLS[%w[collections set_hot]], ns(namespace).merge(
          "collection" => name.to_s, "hot" => { "vectors" => vectors, "text" => text, "fragments" => fragments },
        ), call_opts(opts))
      end

      def warm(name, namespace: nil, **opts)
        rpc(CALLS[%w[collections warm]], ns(namespace).merge("collection" => name.to_s), call_opts(opts))
      end
    end

    # `documents`: write, get, scroll, count and the filter writes.
    class Documents < Base
      # Raw write ops (`WriteOp` JSON). Returns the response; its `token` is
      # the write's consistency token.
      def write(collection, ops, report_existence: false, namespace: nil, **opts)
        rpc(CALLS[%w[documents write]], ns(namespace).merge(
          "collection" => collection.to_s, "ops" => ops, "reportExistence" => report_existence,
        ), call_opts(opts))
      end

      # +docs+: `[{id:, source:, vectors:, sparse_vectors:}]`.
      def upsert(collection, docs, **kw)
        write(collection, docs.map { |d| { "upsert" => Documents.document(d) } }, **kw)
      end

      def delete(collection, ids, **kw)
        write(collection, Array(ids).map { |id| { "delete" => { "id" => Values.document_id(id) } } }, **kw)
      end

      # Sets the keys of +set+ and removes the keys named in +unset+.
      def patch(collection, id, set: nil, unset: nil, vectors: nil, mode: nil, **kw)
        patch = Builders.compact({
          "id" => Values.document_id(id), "mode" => mode&.to_s, "source" => set,
          "deleteKeys" => unset&.map(&:to_s), "vectors" => vectors,
        })
        write(collection, [{ "patch" => patch }], **kw)
      end

      # R13: +docs+ (any Enumerable of `{id:, source:, …}`) in chunks, one
      # in flight at a time. Returns a `BatchResult` (the merged token, the
      # chunks and the rows); a failed chunk raises `BatchError`.
      def write_batch(collection, docs, chunk_rows: Batch::DEFAULT_CHUNK_ROWS, chunk_bytes: Batch::DEFAULT_CHUNK_BYTES, namespace: nil, **opts)
        ops = docs.lazy.map { |d| { "upsert" => Documents.document(d) } }
        Batch.run(ops, chunk_rows: chunk_rows, chunk_bytes: chunk_bytes) do |chunk, key|
          write(collection, chunk, namespace: namespace, **opts, idempotency_key: key)
        end
      end

      # +select+ is a projection Hash (`{source: {include: […]}}`);
      # +consistency+ is `:strong`, `:eventual` or a token.
      def get(collection, ids, select: nil, consistency: nil, namespace: nil, **opts)
        rpc(CALLS[%w[documents get]], ns(namespace).merge(Builders.compact({
          "collection" => collection.to_s, "ids" => Array(ids).map { |id| Values.document_id(id) },
          "select" => select, "consistency" => Search.consistency_json(consistency),
        })), call_opts(opts))
      end

      # One page. +where+ is a typed `Filter`; +filter+ is the server's native
      # filter JSON.
      def scroll(collection, where: nil, filter: nil, after: nil, limit: nil, select: nil, consistency: nil, namespace: nil, **opts)
        rpc(CALLS[%w[documents scroll]], filtered(CALLS[%w[documents scroll]], ns(namespace).merge(Builders.compact({
          "collection" => collection.to_s, "after" => after && Values.document_id(after),
          "limit" => limit, "select" => select, "consistency" => Search.consistency_json(consistency),
        })), where, filter), call_opts(opts))
      end

      # Every document, page after page, until the server stops answering a
      # `next` cursor. An Enumerator.
      def scroll_all(collection, **kw)
        return to_enum(:scroll_all, collection, **kw) unless block_given?

        after = kw.delete(:after)
        loop do
          page = scroll(collection, **kw, after: after)
          page.documents.each { |doc| yield doc }
          break if page.next.nil?

          after = Values.to_ruby(page.next)
        end
      end

      def count(collection, where: nil, filter: nil, consistency: nil, namespace: nil, **opts)
        rpc(CALLS[%w[documents count]], filtered(CALLS[%w[documents count]], ns(namespace).merge(Builders.compact({
          "collection" => collection.to_s, "consistency" => Search.consistency_json(consistency),
        })), where, filter), call_opts(opts))
      end

      def delete_by_filter(collection, where: nil, filter: nil, max_rows: nil, allow_partial: false, cursor: nil, namespace: nil, **opts)
        rpc(CALLS[%w[documents delete_by_filter]], filtered(CALLS[%w[documents delete_by_filter]], ns(namespace).merge(Builders.compact({
          "collection" => collection.to_s, "maxRows" => max_rows, "allowPartial" => allow_partial, "cursor" => cursor,
        })), where, filter), call_opts(opts))
      end

      def patch_by_filter(collection, patch, where: nil, filter: nil, max_rows: nil, allow_partial: false, cursor: nil, namespace: nil, **opts)
        rpc(CALLS[%w[documents patch_by_filter]], filtered(CALLS[%w[documents patch_by_filter]], ns(namespace).merge(Builders.compact({
          "collection" => collection.to_s, "patch" => patch, "maxRows" => max_rows, "allowPartial" => allow_partial, "cursor" => cursor,
        })), where, filter), call_opts(opts))
      end

      # One document as a `WriteDocument`'s JSON.
      def self.document(doc)
        doc = doc.transform_keys(&:to_s)
        Builders.compact({
          "id" => Values.document_id(doc.fetch("id")),
          "source" => doc["source"] || {},
          "vectors" => doc["vectors"],
          "sparseVectors" => doc["sparse_vectors"] || doc["sparseVectors"],
        })
      end

      private

      # The typed `where` goes in the request's `where` field (SDKP-3), which
      # needs protos that declare it; the native `filter` JSON goes as is.
      def filtered(path, request, where, filter)
        unless where.nil?
          binding = Descriptors.binding_for_rpc(path)
          if binding.request.lookup("where").nil?
            raise InvalidArgumentError.new(
              "#{path} takes no typed where in this SDK's protos; pass filter: (the native filter JSON)",
              code: :invalid_argument,
              rpc: path,
            )
          end
          request = request.merge("where" => Builders.json(where))
        end
        request = request.merge("filter" => filter) unless filter.nil?
        request
      end
    end

    # `search`: one search over one collection: text, vector, sparse or
    # hybrid, with filters, fusion, highlights, `select` and pagination.
    class SearchModule < Base
      # +request+ is a `Search` builder or the `SearchRequest`'s JSON, checked
      # (`Search#build`) and sent as built.
      def query(request, **opts)
        rpc(CALLS[%w[search query]], Search.wire(request), call_opts(opts))
      end

      # A text search: a match on one field, a multi-match on several, or a
      # query string.
      def text(collection, text, fields: nil, field: nil, filter: nil, limit: nil, namespace: nil, **opts)
        s = Search.on(collection, namespace: namespace).text(text, fields: fields, field: field)
        s.filter(filter) if filter
        s.limit(limit) if limit
        query(s, **opts)
      end

      # Text plus a vector on +field+, fused (`Fusion.rrf` by default).
      def hybrid(collection, text, vector, field:, fields: nil, fusion: nil, k: nil, filter: nil, limit: nil, namespace: nil, **opts)
        s = Search.on(collection, namespace: namespace)
          .text(text, fields: fields, k: k)
          .vector(field, vector, k: k)
          .fuse(fusion || Fusion.rrf)
        s.filter(filter) if filter
        s.limit(limit) if limit
        query(s, **opts)
      end

      # Every hit, page after page through `search_after`, until a page has
      # fewer than `limit` hits. The request's sort must end with the `pk`
      # tiebreaker, or it is refused here, before any network call.
      def query_all(request, **opts)
        return to_enum(:query_all, request, **opts) unless block_given?

        page_request = Builders.json(request)
        unless Search.tiebreak?(page_request)
          raise InvalidArgumentError.new(
            "query_all needs a sort that ends with the primary key, such as .sort(:score, :pk)",
            code: :invalid_argument,
            reason: :search_after_needs_tiebreak,
            metadata: { "field" => "sort" },
          )
        end
        limit = page_request["limit"] || Search::DEFAULT_K
        loop do
          page = query(page_request, **opts)
          page.hits.each { |hit| yield hit }
          break if page.hits.size < limit || page.hits.empty?

          page_request = page_request.merge("searchAfter" => Values.to_ruby(page.hits.last.sort_values))
        end
      end

    end

    # `vector`: the same RPC as `search.query`, with the vector shape as its
    # required arguments (Q613).
    class VectorModule < Base
      def search(collection, vector, field:, limit: nil, k: nil, filter: nil, namespace: nil, **opts)
        s = Search.on(collection, namespace: namespace).vector(field, vector, k: k)
        s.filter(filter) if filter
        s.limit(limit) if limit
        rpc(CALLS[%w[vector search]], Search.wire(s), call_opts(opts))
      end
    end

    # `graph`: stored property graphs, queried with GQL. Experimental: the
    # package is `unstable`.
    class Graph < Base
      LANGUAGES = {
        gql: "QUERY_LANGUAGE_GQL", cypher: "QUERY_LANGUAGE_CYPHER", sql_pgq: "QUERY_LANGUAGE_SQL_PGQ",
      }.freeze

      def unstable?
        true
      end

      def create(name, mapping: nil, mode: nil, limits: nil, namespace: nil, **opts)
        rpc(CALLS[%w[graph create]], ns(namespace).merge(Builders.compact({
          "name" => name.to_s, "mapping" => mapping, "limits" => limits,
          "mode" => mode && "GRAPH_MODE_#{mode.to_s.upcase}",
        })), call_opts(opts))
      end

      def get(name, namespace: nil, **opts)
        rpc(CALLS[%w[graph get]], ns(namespace).merge("name" => name.to_s), call_opts(opts))
      end

      def list(page_size: nil, page_token: nil, namespace: nil, **opts)
        rpc(CALLS[%w[graph list]], ns(namespace).merge(Builders.compact({ "pageSize" => page_size, "pageToken" => page_token })), call_opts(opts))
      end

      def list_all(page_size: nil, namespace: nil, **opts)
        paged(CALLS[%w[graph list]], ns(namespace).merge(Builders.compact({ "pageSize" => page_size })), "graphs", call_opts(opts))
      end

      # +graph+ is the `Graph` JSON (with `namespace` and `name`), and
      # +update_mask+ the paths it changes.
      def update(graph, update_mask:, expected_version: nil, **opts)
        graph = graph.transform_keys(&:to_s)
        graph["namespace"] ||= @client.namespace
        rpc(CALLS[%w[graph update]], Builders.compact({
          "graph" => graph, "updateMask" => Array(update_mask).join(","),
          "expectedVersion" => expected_version&.to_s,
        }), call_opts(opts))
      end

      # Answers an `Operation`.
      def delete(name, namespace: nil, **opts)
        rpc(CALLS[%w[graph delete]], ns(namespace).merge("name" => name.to_s), call_opts(opts))
      end

      def schema(name, namespace: nil, **opts)
        rpc(CALLS[%w[graph schema]], ns(namespace).merge("name" => name.to_s), call_opts(opts))
      end

      # One statement (GQL by default). +parameters+ are Ruby values.
      def query(graph, statement, parameters: {}, read_only: false, language: :gql, consistency: nil,
        max_rows: nil, namespace: nil, **opts)
        rpc(CALLS[%w[graph query]], execute_request(graph, statement, parameters, read_only, language, consistency, max_rows, namespace), call_opts(opts))
      end

      # +statements+: `[{statement:, parameters:}]`, run in one batch.
      def query_batch(graph, statements, atomic: true, language: :gql, namespace: nil, **opts)
        list = statements.map do |s|
          s = s.transform_keys(&:to_s)
          { "statement" => s.fetch("statement"), "parameters" => params(s["parameters"] || {}) }
        end
        rpc(CALLS[%w[graph query_batch]], ns(namespace).merge(
          "graph" => graph.to_s, "statements" => list, "atomic" => atomic, "language" => language_name(language),
        ), call_opts(opts))
      end

      # A server stream of `ResultChunk`s (gRPC-only on Connect-unary transports).
      def query_stream(graph, statement, parameters: {}, read_only: true, language: :gql, chunk_rows: nil, namespace: nil, **opts)
        inner = execute_request(graph, statement, parameters, read_only, language, nil, nil, namespace || @client.namespace)
        rpc(CALLS[%w[graph query_stream]], Builders.compact({ "request" => inner, "chunkRows" => chunk_rows }), call_opts(opts))
      end

      def explain(graph, statement, parameters: {}, profile: false, language: :gql, namespace: nil, **opts)
        rpc(CALLS[%w[graph explain]], ns(namespace).merge(
          "graph" => graph.to_s, "statement" => statement.to_s, "parameters" => params(parameters),
          "profile" => profile, "language" => language_name(language),
        ), call_opts(opts))
      end

      # An `ExecuteResponse`'s rows as Hashes, column name to Ruby value.
      def self.records(response)
        rows = response.rows
        return [] if rows.nil?

        columns = rows.columns.to_a
        rows.rows.map { |row| columns.zip(row.values.map { |v| Values.from_graph_value(v) }).to_h }
      end

      private

      def execute_request(graph, statement, parameters, read_only, language, consistency, max_rows, namespace)
        ns(namespace).merge(Builders.compact({
          "graph" => graph.to_s, "statement" => statement.to_s, "language" => language_name(language),
          "parameters" => params(parameters), "readOnly" => read_only,
          "consistency" => graph_consistency(consistency), "maxRows" => max_rows,
        }))
      end

      def params(parameters)
        parameters.to_h { |k, v| [k.to_s, Values.graph_value(v)] }
      end

      def language_name(language)
        LANGUAGES.fetch(language.to_sym) { language.to_s }
      end

      def graph_consistency(level)
        case level
        when nil then nil
        when :strong then { "strong" => {} }
        when :eventual then { "eventual" => {} }
        else { "atLeast" => level.to_s }
        end
      end
    end
  end

  module App
    # `sql`: one read-only statement over a namespace's collections, with
    # `$1…$n` parameters. SQL reads see the session's collection writes.
    class Sql < Base
      # The answer's `rows` are lists in `columns` order; see `.records`.
      def query(sql, params: [], max_rows: nil, consistency: nil, namespace: nil, **opts)
        rpc(CALLS[%w[sql query]], request(sql, params, max_rows, consistency, namespace), call_opts(opts))
      end

      # The same statement, answered as Arrow IPC bytes.
      def query_arrow(sql, params: [], max_rows: nil, consistency: nil, namespace: nil, **opts)
        rpc(CALLS[%w[sql query_arrow]], request(sql, params, max_rows, consistency, namespace), call_opts(opts)).arrow_ipc
      end

      # A `QueryResponse`'s rows as Hashes, column name to value.
      def self.records(response)
        names = response.columns.map(&:name)
        response.rows.map { |row| names.zip(Values.to_ruby(row)).to_h }
      end

      private

      def request(sql, params, max_rows, consistency, namespace)
        ns(namespace).merge(Builders.compact({
          "sql" => sql.to_s, "params" => params.empty? ? nil : params,
          "maxRows" => max_rows, "consistency" => Search.consistency_json(consistency),
        }))
      end
    end

    # One fetched record, decoded: `offset`, `key` and `value` (binary
    # Strings, or nil), `headers` and `timestamp_ms`.
    StreamRecord = Struct.new(:offset, :key, :value, :headers, :timestamp_ms, keyword_init: true) do
      def json
        value && JSON.parse(value)
      end
    end

    # `streams`: a namespace's logs. Records are bytes on the wire; the module
    # takes Strings (bytes as they are) or other values (sent as JSON) and
    # answers bytes.
    class Streams < Base
      def create(name, partitions: 1, max_age_ms: nil, max_bytes: nil, namespace: nil, **opts)
        retention = Builders.compact({ "maxAgeMs" => max_age_ms&.to_s, "maxBytes" => max_bytes&.to_s })
        rpc(CALLS[%w[streams create]], ns(namespace).merge(Builders.compact({
          "name" => name.to_s, "partitions" => partitions.zero? ? nil : partitions,
          "retention" => retention.empty? ? nil : retention,
        })), call_opts(opts))
      end

      def get(stream, namespace: nil, **opts)
        rpc(CALLS[%w[streams get]], ns(namespace).merge("stream" => stream.to_s), call_opts(opts))
      end

      def list(page_size: nil, page_token: nil, namespace: nil, **opts)
        rpc(CALLS[%w[streams list]], ns(namespace).merge(Builders.compact({ "pageSize" => page_size, "pageToken" => page_token })), call_opts(opts))
      end

      def list_all(page_size: nil, namespace: nil, **opts)
        paged(CALLS[%w[streams list]], ns(namespace).merge(Builders.compact({ "pageSize" => page_size })), "streams", call_opts(opts))
      end

      # Appends `records` to one partition. A record is a value, or
      # `{key:, value:, headers: {name => value}}` with an optional key
      # (SDKP-25). With `producer_id` and `producer_seq` a repeat is
      # deduplicated (`duplicate`).
      def produce(stream, records, partition: 0, producer_id: nil, producer_seq: nil, namespace: nil, **opts)
        rpc(CALLS[%w[streams produce]], ns(namespace).merge(Builders.compact({
          "stream" => stream.to_s, "partition" => partition.zero? ? nil : partition,
          "records" => records.map { |r| Streams.record(r) },
          "producerId" => producer_id, "producerSeq" => producer_seq&.to_s,
        })), call_opts(opts))
      end

      # Records at and after `offset`; `.records` decodes the answer.
      def fetch(stream, partition: 0, offset: 0, max_records: nil, max_bytes: nil, max_wait_ms: nil, namespace: nil, **opts)
        rpc(CALLS[%w[streams fetch]], ns(namespace).merge(Builders.compact({
          "stream" => stream.to_s, "partition" => partition.zero? ? nil : partition,
          "offset" => offset.zero? ? nil : offset.to_s, "maxBytes" => max_bytes,
          "maxRecords" => max_records, "maxWaitMs" => max_wait_ms,
        })), call_opts(opts))
      end

      # CloudEvents (Hashes) appended as one JSON batch.
      def produce_events(stream, events, partition: nil, namespace: nil, **opts)
        rpc(CALLS[%w[streams produce_events]], ns(namespace).merge(Builders.compact({
          "stream" => stream.to_s, "partition" => partition,
          "jsonBatch" => [JSON.generate(events)].pack("m0"),
        })), call_opts(opts))
      end

      # CloudEvents at and after `offset`, decoded from the answer's JSON batch.
      def fetch_events(stream, partition: 0, offset: 0, max_bytes: nil, max_wait_ms: nil, namespace: nil, **opts)
        answer = rpc(CALLS[%w[streams fetch_events]], ns(namespace).merge(Builders.compact({
          "stream" => stream.to_s, "partition" => partition.zero? ? nil : partition,
          "offset" => offset.zero? ? nil : offset.to_s, "maxBytes" => max_bytes, "maxWaitMs" => max_wait_ms,
        })), call_opts(opts))
        answer.json_batch.empty? ? [] : JSON.parse(answer.json_batch)
      end

      # R14: every record from `from_offset` on, as a loop over `fetch`:
      # enumerate it, `cancel` it, resume from `offset`.
      def subscribe(stream, partition: 0, from_offset: 0, max_records: nil, max_wait_ms: nil, namespace: nil, **opts)
        Subscription.new(from_offset: from_offset) do |offset|
          Streams.records(fetch(stream, partition: partition, offset: offset, max_records: max_records,
            max_wait_ms: max_wait_ms, namespace: namespace, **opts))
        end
      end

      # A `FetchResponse`'s records, decoded.
      def self.records(response)
        response.records.map do |r|
          StreamRecord.new(
            offset: r.offset, key: r.key, value: r.value,
            headers: r.headers.to_h { |h| [h.key, h.value] }, timestamp_ms: r.timestamp_ms,
          )
        end
      end

      # One record as a `Record`'s JSON (bytes as base64).
      def self.record(record)
        if record.is_a?(Hash) && (record.key?(:value) || record.key?("value") || record.key?(:key) || record.key?("key"))
          r = record.transform_keys(&:to_s)
          headers = (r["headers"] || {}).map { |k, v| Builders.compact({ "key" => k.to_s, "value" => v.nil? ? nil : b64(v) }) }
          Builders.compact({
            "key" => r["key"].nil? ? nil : b64(r["key"]),
            "value" => r["value"].nil? ? nil : b64(r["value"]),
            "headers" => headers.empty? ? nil : headers,
          })
        else
          { "value" => b64(record) }
        end
      end

      def self.b64(value)
        [value.is_a?(String) ? value : JSON.generate(value)].pack("m0")
      end
    end
  end

  # `client.collection("notes")`: every document and search call with the
  # collection (and namespace) filled in. It holds no state of its own.
  class CollectionHandle
    attr_reader :name, :namespace

    def initialize(client, name, namespace: nil)
      @client = client
      @name = name.to_s
      @namespace = namespace
    end

    def upsert(docs, **kw) = @client.documents.upsert(@name, docs, **with_ns(kw))
    def get(ids, **kw) = @client.documents.get(@name, ids, **with_ns(kw))
    def delete(ids, **kw) = @client.documents.delete(@name, ids, **with_ns(kw))
    def patch(id, **kw) = @client.documents.patch(@name, id, **with_ns(kw))
    def write_batch(docs, **kw) = @client.documents.write_batch(@name, docs, **with_ns(kw))
    def count(**kw) = @client.documents.count(@name, **with_ns(kw))
    def scroll_all(**kw, &block) = @client.documents.scroll_all(@name, **with_ns(kw), &block)
    def vector_search(vector, **kw) = @client.vector.search(@name, vector, **with_ns(kw))
    def drop(**kw) = @client.collections.drop(@name, **with_ns(kw))
    def info(**kw) = @client.collections.get(@name, **with_ns(kw))

    # +builder+ is a `Search` (its collection is set to this one), or a block
    # that receives a fresh `Search.on(name)`.
    def search(builder = nil, **opts)
      builder ||= Search.on(@name)
      builder = builder.dup
      builder.collection(@name)
      builder.namespace(@namespace) if @namespace && builder.namespace_name.nil?
      yield builder if block_given?
      @client.search.query(builder, **opts)
    end

    private

    def with_ns(kw)
      @namespace.nil? || kw.key?(:namespace) ? kw : kw.merge(namespace: @namespace)
    end
  end
end

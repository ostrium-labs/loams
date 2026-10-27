# frozen_string_literal: true

require "loams/errors"

module Loams
  # A consistency token (runtime contract R4): `v1:s<stream>/p<partition>@<offset>,…`,
  # the encoding `crates/loams-collection/src/token.rs` defines.
  #
  # A write answers with one and a read accepts one as `at_least`, so a caller
  # that just wrote can read its own write. Two tokens merge by keeping the
  # largest offset per `(stream, partition)`, which is what lets a session fold
  # every answer it sees into one token.
  class ConsistencyToken
    PREFIX = "v1:"
    U32_MAX = 0xFFFF_FFFF
    U64_MAX = 0xFFFF_FFFF_FFFF_FFFF

    # The `(stream, partition, offset)` triples, in the order the token
    # spells them. `merge` (and `normalized`) sort them, one per
    # `(stream, partition)`.
    attr_reader :items

    # Parses +text+, raising `InvalidArgumentError` (client-side, no network
    # call) when it is not a `v1:` token.
    def self.parse(text)
      raise invalid(text) unless text.is_a?(String) && text.start_with?(PREFIX)

      body = text[PREFIX.length..]
      return new([]) if body.empty?

      items = body.split(",", -1).map do |item|
        parse_item(item) || raise(invalid(text))
      end
      new(items)
    end

    # Whether +text+ parses.
    def self.valid?(text)
      parse(text)
      true
    rescue LoamsError
      false
    end

    def initialize(items = [])
      @items = items.map { |i| i.dup.freeze }.freeze
      freeze
    end

    # The same token sorted by stream then partition, one item per pair (the
    # largest offset).
    def normalized
      self.class.new(normalize(@items))
    end

    # The token's text, `v1:` followed by the triples.
    def to_s
      PREFIX + @items.map { |s, p, o| "s#{s}/p#{p}@#{o}" }.join(",")
    end

    # A new token holding the larger offset of the two for every
    # `(stream, partition)`.
    def merge(other)
      other = self.class.parse(other) if other.is_a?(String)
      self.class.new(normalize(@items + other.items))
    end

    # The offset this token holds for one partition, or nil.
    def offset(stream, partition)
      @items.select { |s, p, _| s == stream && p == partition }.map(&:last).max
    end

    def empty?
      @items.empty?
    end

    def ==(other)
      other.is_a?(ConsistencyToken) && other.items == @items
    end
    alias eql? ==

    def hash
      @items.hash
    end

    def inspect
      "#<Loams::ConsistencyToken #{self}>"
    end

    def self.parse_item(item)
      return nil unless item.start_with?("s")

      stream, rest = item[1..].split("/p", 2)
      return nil if rest.nil?

      partition, offset = rest.split("@", 2)
      return nil if offset.nil?

      s = canonical(stream, U64_MAX)
      p = canonical(partition, U32_MAX)
      o = canonical(offset, U64_MAX)
      return nil if s.nil? || p.nil? || o.nil?

      [s, p, o]
    end
    private_class_method :parse_item

    # A decimal with no sign, no leading zero (except `0`) and in range.
    def self.canonical(text, max)
      return nil unless text.match?(/\A(0|[1-9][0-9]*)\z/)

      n = Integer(text, 10)
      n <= max ? n : nil
    end
    private_class_method :canonical

    def self.invalid(text)
      InvalidArgumentError.new("invalid consistency token #{text.inspect}", code: :invalid_argument, reason: :invalid_consistency_token)
    end
    private_class_method :invalid

    private

    def normalize(items)
      items.sort.each_with_object([]) do |item, out|
        if out.last && out.last[0] == item[0] && out.last[1] == item[1]
          out[-1] = item
        else
          out << item
        end
      end.freeze
    end
  end

  # The session store (R4, `session_consistency: true`): every token an
  # answer carries is merged in, and every read that leaves `consistency`
  # unset is sent with `at_least` set to the session's token.
  #
  # One token per proto package: a graph's tokens name the graph's own
  # stream and a graph read refuses a token of another stream, so the
  # collection and graph sessions are kept apart.
  class SessionStore
    # The response fields a token is read from.
    TOKEN_FIELDS = %w[token read_token consistency_token].freeze

    def initialize
      @tokens = {}
      @lock = Mutex.new
    end

    # The session token for +scope+ (a proto package), or nil.
    def token(scope = "loams.collection.v1")
      @lock.synchronize { @tokens[scope] }
    end

    # Merges +text+ into the session for +scope+. A token that does not
    # parse is ignored: the session never holds a token it cannot merge.
    def observe(scope, text)
      return if text.nil? || text.empty?
      return unless ConsistencyToken.valid?(text)

      parsed = ConsistencyToken.parse(text)
      @lock.synchronize do
        current = @tokens[scope]
        @tokens[scope] = current ? current.merge(parsed) : parsed
      end
    end

    # Reads every token field off +message+ and merges it in.
    def observe_message(scope, message)
      return if message.nil? || !message.respond_to?(:class) || !message.class.respond_to?(:descriptor)

      TOKEN_FIELDS.each do |name|
        field = message.class.descriptor.lookup(name)
        next if field.nil? || field.type != :string || field.label == :repeated

        observe(scope, message[name])
      end
    end

    # Sets `consistency.at_least` on +request+ when it has a `consistency`
    # field the caller left unset and the session holds a token.
    def apply(scope, request)
      return request unless request.respond_to?(:class) && request.class.respond_to?(:descriptor)

      field = request.class.descriptor.lookup("consistency")
      return request if field.nil? || field.type != :message
      return request unless request[field.name].nil?

      current = token(scope)
      return request if current.nil? || current.empty?

      sub = field.subtype.msgclass.new
      return request if sub.class.descriptor.lookup("at_least").nil?

      sub["at_least"] = current.to_s
      request[field.name] = sub
      request
    end
  end
end

# frozen_string_literal: true

module Loams
  # R14: `streams.subscribe`, a loop over unary `Fetch` (SDKP-2), so it works
  # on Connect-unary and through any proxy.
  #
  # The next offset is the last record's offset + 1. An empty fetch backs off
  # 100 ms, doubling up to 1 s, and a non-empty one resets the backoff.
  # `offset` is where the next fetch starts, so a caller can resume from it;
  # `cancel` stops the loop at the next fetch boundary.
  #
  #   sub = client.streams.subscribe("events", 0, from_offset: 1)
  #   sub.each { |record| … ; sub.cancel if done }
  class Subscription
    include Enumerable

    INITIAL_BACKOFF = 0.1
    MAX_BACKOFF = 1.0

    attr_reader :offset

    # +fetch+ is called with an offset and answers the records at and after
    # it, each responding to `offset` (or a Hash with `"offset"`).
    def initialize(from_offset: 0, sleeper: nil, &fetch)
      raise ArgumentError, "a subscription needs a fetch block" unless fetch

      @offset = Integer(from_offset)
      @fetch = fetch
      @sleeper = sleeper || ->(seconds) { sleep(seconds) }
      @cancelled = false
    end

    def cancel
      @cancelled = true
    end

    def cancelled?
      @cancelled
    end

    def each
      return to_enum(:each) unless block_given?

      backoff = INITIAL_BACKOFF
      until @cancelled
        records = Array(@fetch.call(@offset))
        if records.empty?
          @sleeper.call(backoff)
          backoff = [backoff * 2, MAX_BACKOFF].min
          next
        end
        backoff = INITIAL_BACKOFF
        records.each do |record|
          break if @cancelled

          @offset = Integer(record_offset(record)) + 1
          yield record
        end
      end
      self
    end

    private

    def record_offset(record)
      if record.respond_to?(:offset)
        record.offset
      elsif record.is_a?(Hash)
        record["offset"] || record[:offset]
      else
        raise ArgumentError, "a fetched record has no offset"
      end
    end
  end
end

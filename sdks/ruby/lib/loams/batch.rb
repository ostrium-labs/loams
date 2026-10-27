# frozen_string_literal: true

require "json"
require "loams/errors"
require "loams/idempotency"
require "loams/consistency"

module Loams
  # What a `write_batch` applied: the merged consistency token (a String, nil
  # when no chunk answered one), the chunks and the rows.
  BatchResult = Struct.new(:token, :chunks, :rows, keyword_init: true)

  # A `write_batch` stopped at a failed chunk (R13): the chunks before it were
  # applied, and `token` is their merged consistency token.
  class BatchError < LoamsError
    attr_reader :chunks_applied, :rows_applied, :token, :cause_error

    def initialize(cause, chunks_applied:, rows_applied:, token:)
      @chunks_applied = chunks_applied
      @rows_applied = rows_applied
      @token = token
      @cause_error = cause
      code = cause.respond_to?(:code) ? cause.code : :unknown
      metadata = { "chunks_applied" => chunks_applied.to_s, "rows_applied" => rows_applied.to_s }
      cause_reason = cause.respond_to?(:reason) && cause.reason
      metadata["cause_reason"] = cause_reason.to_s if cause_reason
      super(
        "write_batch stopped at chunk #{chunks_applied + 1}: #{chunks_applied} chunks (#{rows_applied} operations) " \
        "were applied before it; #{cause.message}",
        # SDKP-23: registered under `aborted`; the failed chunk's error is the cause.
        code: :aborted,
        reason: :batch_partially_applied,
        metadata: metadata,
        http_status: cause.respond_to?(:http_status) ? cause.http_status : nil,
        rpc: cause.respond_to?(:rpc) ? cause.rpc : nil,
      )
    end
  end

  # R13: a large write as a sequence of `WriteDocuments` chunks.
  #
  # One chunk is in flight at a time. A chunk closes at +chunk_rows+ ops or
  # when the next op would take it past +chunk_bytes+ of JSON. Each chunk gets
  # its own idempotency key, which the invoker reuses on that chunk's retries,
  # so a retried chunk is applied once. The first chunk that fails stops the
  # batch with a `BatchError`.
  module Batch
    DEFAULT_CHUNK_ROWS = 10_000
    DEFAULT_CHUNK_BYTES = 4 * 1024 * 1024

    # +ops+ is any Enumerable of `WriteOp` JSON Hashes; +write+ is called with
    # each chunk's ops and its idempotency key and answers the response.
    # Returns a `BatchResult`.
    def self.run(ops, chunk_rows: DEFAULT_CHUNK_ROWS, chunk_bytes: DEFAULT_CHUNK_BYTES, &write)
      raise ArgumentError, "chunk_rows must be positive" unless chunk_rows.positive?
      raise ArgumentError, "chunk_bytes must be positive" unless chunk_bytes.positive?

      token = nil
      applied = 0
      rows = 0
      chunk = []
      size = 0

      flush = lambda do
        return if chunk.empty?

        begin
          res = write.call(chunk, Idempotency.uuidv7)
        rescue LoamsError => e
          raise BatchError.new(e, chunks_applied: applied, rows_applied: rows, token: token&.to_s)
        end
        text = res.respond_to?(:token) ? res.token : res["token"]
        if text && !text.empty? && ConsistencyToken.valid?(text)
          parsed = ConsistencyToken.parse(text)
          token = token ? token.merge(parsed) : parsed.normalized
        end
        applied += 1
        rows += chunk.size
        chunk = []
        size = 0
      end

      ops.each do |op|
        bytes = JSON.generate(op).bytesize
        flush.call if !chunk.empty? && (chunk.size >= chunk_rows || size + bytes > chunk_bytes)
        chunk << op
        size += bytes
      end
      flush.call
      BatchResult.new(token: token&.to_s, chunks: applied, rows: rows)
    end
  end
end

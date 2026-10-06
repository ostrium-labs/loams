# frozen_string_literal: true

require "loams/envelopes"
require "loams/codec"

module Loams
  class ServerStreamHandle
    attr_reader :frame_kinds, :heartbeats, :reconnects

    def initialize(initial_iterator, &reconnect_proc)
      @initial_iterator = initial_iterator
      @reconnect_proc = reconnect_proc
      @frame_kinds = []
      @heartbeats = 0
      @reconnects = 0
    end

    def messages
      return to_enum(:messages) unless block_given?

      last_cursor = nil
      current_enum = @initial_iterator

      loop do
        begin
          current_enum.each do |frame|
            if frame.end_stream?
              # Connect end frame: contains error if status != 0
              handle_end_frame(frame.payload)
              return
            elsif frame.trailers?
              # gRPC-Web trailers
              handle_trailers_frame(frame.payload)
              return
            end

            # Data frame
            msg = decode_message(frame.payload)
            kind = detect_kind(msg)
            @frame_kinds << kind

            cursor = detect_cursor(msg)
            last_cursor = cursor if cursor && !cursor.empty?

            if kind == "heartbeat"
              @heartbeats += 1
              next
            end

            yield msg
          end
          return
        rescue LoamsError => e
          if @reconnect_proc && e.retryable?
            @reconnects += 1
            current_enum = @reconnect_proc.call(last_cursor)
          else
            raise e
          end
        rescue StandardError => e
          if @reconnect_proc
            @reconnects += 1
            current_enum = @reconnect_proc.call(last_cursor)
          else
            raise e
          end
        end
      end
    end

    def count_frame(kind)
      @frame_kinds << kind
    end

    def count_heartbeat
      @heartbeats += 1
    end

    def record_reconnect
      @reconnects += 1
    end

    private

    def decode_message(payload)
      @decode_proc ? @decode_proc.call(payload) : payload
    end

    def detect_kind(msg)
      return "data" unless msg

      h = msg.respond_to?(:to_h) ? msg.to_h : (msg.is_a?(Hash) ? msg : {})
      [:snapshot, :heartbeat, :upsert, :remove, :update, :delete].each do |k|
        val = h[k] || h[k.to_s]
        return k.to_s if !val.nil?
      end
      "data"
    end

    def detect_cursor(msg)
      return nil unless msg

      h = msg.respond_to?(:to_h) ? msg.to_h : (msg.is_a?(Hash) ? msg : {})
      c = h[:cursor] || h["cursor"] || h[:resume_token] || h["resume_token"]
      c.to_s.empty? ? nil : c.to_s
    end

    def handle_end_frame(payload)
      return if payload.nil? || payload.empty?

      parsed = JSON.parse(payload) rescue nil
      if parsed && parsed["error"]
        raise ErrorParser.parse_connect_error(payload, 200)
      end
    end

    def handle_trailers_frame(payload)
      return if payload.nil? || payload.empty?

      headers = {}
      payload.each_line do |line|
        if line.include?(":")
          k, v = line.split(":", 2)
          headers[k.strip.downcase] = v.strip
        end
      end

      status = headers["grpc-status"]
      if status && status.to_i != 0
        raise ErrorParser.parse_grpc_trailers(headers)
      end
    end
  end
end

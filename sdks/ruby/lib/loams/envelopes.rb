# frozen_string_literal: true

module Loams
  module Envelopes
    FLAG_DATA = 0x00
    FLAG_END_STREAM = 0x02
    FLAG_GRPC_TRAILERS = 0x80

    Frame = Struct.new(:flags, :payload) do
      def data?
        flags == FLAG_DATA
      end

      def end_stream?
        (flags & FLAG_END_STREAM) != 0
      end

      def trailers?
        (flags & FLAG_GRPC_TRAILERS) != 0
      end
    end

    # Wraps payload into 5-byte envelope frame (1 byte flags, 4 bytes big-endian length)
    def self.wrap(payload, flags = FLAG_DATA)
      payload = payload.b
      [flags, payload.bytesize].pack("CN") + payload
    end

    # Splits a byte buffer containing one or more frames
    def self.split(bytes)
      return [] if bytes.nil? || bytes.empty?

      bytes = bytes.b
      frames = []
      pos = 0
      len = bytes.bytesize

      while pos + 5 <= len
        flags, payload_len = bytes.byteslice(pos, 5).unpack("CN")
        pos += 5
        if pos + payload_len > len
          raise ArgumentError, "Frame length #{payload_len} exceeds remaining bytes #{len - pos}"
        end

        payload = bytes.byteslice(pos, payload_len)
        pos += payload_len
        frames << Frame.new(flags, payload)
      end

      frames
    end
  end
end

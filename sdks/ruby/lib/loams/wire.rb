# The protobuf wire format, read and written by hand.
#
# Two things need it and neither is the message codec: the **custom
# options** the protos carry (the `loams.options.v1.module` and
# `loams.options.v1.facade` annotations, which `buf build` writes as
# extension fields on `ServiceOptions` and `MethodOptions` and which the
# descriptor set therefore carries as unknown fields), and the **Connect
# and gRPC-Web frames** a stream is made of. Both are length-delimited
# fields with a varint tag, so one reader serves them.
#
# The reader is deliberately strict: a tag that names a wire type it
# cannot skip is a refusal, not a guess, because a mis-read byte here is
# a module name invented out of nothing.
module Loams
  module Wire
    # A varint read from +bytes+ at +pos+. Returns [value, next_pos].
    def self.read_varint(bytes, pos)
      value = 0
      shift = 0
      loop do
        b = bytes.getbyte(pos)
        raise ArgumentError, "truncated varint at #{pos}" if b.nil?
        pos += 1
        value |= (b & 0x7f) << shift
        break if (b & 0x80).zero?
        shift += 7
        raise ArgumentError, "varint too long at #{pos}" if shift > 63
      end
      [value, pos]
    end

    # A varint written to +io+.
    def self.write_varint(io, value)
      until value < 0x80
        io.write(((value & 0x7f) | 0x80).chr)
        value >>= 7
      end
      io.write(value.chr)
    end

    # The tag for a field: the field number in the high bits, the wire
    # type in the low three.
    def self.tag(field_number, wire_type)
      (field_number << 3) | wire_type
    end

    # Every field in +bytes+, as [field_number, wire_type, value] where
    # value is an Integer for a varint and a String for a length-delimited
    # field. Unknown wire types (1, 5 — fixed64, fixed32) are skipped by
    # length only when the caller can say how long they are, which it
    # cannot from a tag alone, so they are refused.
    def self.each_field(bytes)
      return to_enum(:each_field, bytes) unless block_given?
      pos = 0
      while pos < bytes.bytesize
        tag, pos = read_varint(bytes, pos)
        field = tag >> 3
        wire_type = tag & 7
        case wire_type
        when 0
          value, pos = read_varint(bytes, pos)
          yield field, wire_type, value
        when 2
          len, pos = read_varint(bytes, pos)
          raise ArgumentError, "length #{len} overruns #{bytes.bytesize} at #{pos}" if pos + len > bytes.bytesize
          value = bytes.byteslice(pos, len)
          pos += len
          yield field, wire_type, value
        when 1
          raise ArgumentError, "fixed64 field #{field} cannot be skipped" if pos + 8 > bytes.bytesize
          pos += 8
        when 5
          raise ArgumentError, "fixed32 field #{field} cannot be skipped" if pos + 4 > bytes.bytesize
          pos += 4
        else
          raise ArgumentError, "wire type #{wire_type} on field #{field} is a group, which this corpus does not carry"
        end
      end
    end

    # The first length-delimited field +field_number+ in +bytes+, or nil.
    def self.find_bytes(bytes, field_number)
      each_field(bytes) do |field, wire_type, value|
        return value if field == field_number && wire_type == 2
      end
      nil
    end

    # The first varint field +field_number+ in +bytes+, or nil.
    def self.find_varint(bytes, field_number)
      each_field(bytes) do |field, wire_type, value|
        return value if field == field_number && wire_type == 0
      end
      nil
    end

    # A length-delimited field written to +io+.
    def self.write_bytes(io, field_number, payload)
      write_varint(io, tag(field_number, 2))
      write_varint(io, payload.bytesize)
      io.write(payload)
    end

    # A varint field written to +io+.
    def self.write_varint_field(io, field_number, value)
      write_varint(io, tag(field_number, 0))
      write_varint(io, value)
    end
  end
end

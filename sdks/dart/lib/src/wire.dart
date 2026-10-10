import 'dart:typed_data';

class WireField {
  final int field;
  final int wireType;
  final dynamic value;

  WireField({required this.field, required this.wireType, required this.value});
}

class Wire {
  static (int value, int nextPos) readVarint(Uint8List bytes, int pos) {
    int value = 0;
    int shift = 0;
    final len = bytes.length;

    while (true) {
      if (pos >= len) {
        throw ArgumentError('truncated varint at pos $pos');
      }
      final b = bytes[pos++];
      value |= (b & 0x7F) << shift;
      if ((b & 0x80) == 0) {
        break;
      }
      shift += 7;
      if (shift > 63) {
        throw ArgumentError('varint too long at pos $pos');
      }
    }

    return (value, pos);
  }

  static Uint8List writeVarint(int value) {
    final builder = BytesBuilder();
    while (value >= 0x80) {
      builder.addByte((value & 0x7F) | 0x80);
      value >>= 7;
    }
    builder.addByte(value);
    return builder.toBytes();
  }

  static int tag(int fieldNumber, int wireType) {
    return (fieldNumber << 3) | wireType;
  }

  static Uint8List writeBytes(int fieldNumber, Uint8List payload) {
    final builder = BytesBuilder();
    builder.add(writeVarint(tag(fieldNumber, 2)));
    builder.add(writeVarint(payload.length));
    builder.add(payload);
    return builder.toBytes();
  }

  static Uint8List writeVarintField(int fieldNumber, int value) {
    final builder = BytesBuilder();
    builder.add(writeVarint(tag(fieldNumber, 0)));
    builder.add(writeVarint(value));
    return builder.toBytes();
  }

  static List<WireField> eachField(Uint8List bytes) {
    final fields = <WireField>[];
    int pos = 0;
    final len = bytes.length;

    while (pos < len) {
      final (tagVal, nextPos) = readVarint(bytes, pos);
      pos = nextPos;
      final field = tagVal >> 3;
      final wireType = tagVal & 7;

      switch (wireType) {
        case 0:
          final (val, p) = readVarint(bytes, pos);
          pos = p;
          fields.add(WireField(field: field, wireType: wireType, value: val));
          break;
        case 2:
          final (payloadLen, p) = readVarint(bytes, pos);
          pos = p;
          if (pos + payloadLen > len) {
            throw ArgumentError('field $field length $payloadLen overruns $len at $pos');
          }
          final payload = Uint8List.sublistView(bytes, pos, pos + payloadLen);
          pos += payloadLen;
          fields.add(WireField(field: field, wireType: wireType, value: payload));
          break;
        case 1:
          if (pos + 8 > len) {
            throw ArgumentError('field $field 64-bit truncated');
          }
          pos += 8;
          break;
        case 5:
          if (pos + 4 > len) {
            throw ArgumentError('field $field 32-bit truncated');
          }
          pos += 4;
          break;
        default:
          throw ArgumentError('unsupported wire type $wireType for field $field');
      }
    }

    return fields;
  }

  static Uint8List? findBytes(Uint8List bytes, int fieldNumber) {
    for (final f in eachField(bytes)) {
      if (f.field == fieldNumber && f.wireType == 2) {
        return f.value as Uint8List;
      }
    }
    return null;
  }

  static int? findVarint(Uint8List bytes, int fieldNumber) {
    for (final f in eachField(bytes)) {
      if (f.field == fieldNumber && f.wireType == 0) {
        return f.value as int;
      }
    }
    return null;
  }
}

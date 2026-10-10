import 'dart:typed_data';

class Frame {
  final int flags;
  final Uint8List payload;

  Frame({required this.flags, required this.payload});

  bool get isMessage => flags == 0;
  bool get isTrailer => (flags & 0x02) != 0 || (flags & 0x80) != 0;
}

class Envelopes {
  static Uint8List pack(Uint8List payload, {int flags = 0}) {
    final builder = BytesBuilder();
    builder.addByte(flags);
    final bdata = ByteData(4);
    bdata.setUint32(0, payload.length, Endian.big);
    builder.add(bdata.buffer.asUint8List());
    builder.add(payload);
    return builder.toBytes();
  }

  static List<Frame> split(Uint8List bytes) {
    final frames = <Frame>[];
    int pos = 0;
    final len = bytes.length;

    while (pos < len) {
      if (pos + 5 > len) break;
      final flags = bytes[pos];
      final bdata = ByteData.sublistView(bytes, pos + 1, pos + 5);
      final payloadLen = bdata.getUint32(0, Endian.big);
      pos += 5;

      if (pos + payloadLen > len) break;
      final payload = Uint8List.sublistView(bytes, pos, pos + payloadLen);
      pos += payloadLen;

      frames.add(Frame(flags: flags, payload: payload));
    }

    return frames;
  }
}

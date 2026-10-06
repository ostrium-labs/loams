import 'dart:math';

class Idempotency {
  static final Random _random = Random.secure();

  static String mintKey() {
    final timestamp = DateTime.now().millisecondsSinceEpoch;
    final timeHex = timestamp.toRadixString(16).padLeft(12, '0');

    final part1 = timeHex.substring(0, 8);
    final part2 = timeHex.substring(8, 12);

    final r1 = _random.nextInt(0xFFF).toRadixString(16).padLeft(3, '0');
    final part3 = '7$r1';

    final r2 = (_random.nextInt(0x3FFF) | 0x8000).toRadixString(16).padLeft(4, '0');
    final part4 = r2;

    final r3 = _random.nextInt(0xFFFFFF).toRadixString(16).padLeft(6, '0');
    final r4 = _random.nextInt(0xFFFFFF).toRadixString(16).padLeft(6, '0');
    final part5 = '$r3$r4';

    return '$part1-$part2-$part3-$part4-$part5';
  }
}

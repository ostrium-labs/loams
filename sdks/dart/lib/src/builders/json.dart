/// Drops the null values of [map], so an unset optional field is absent.
Map<String, dynamic> compact(Map<String, dynamic> map) => Map.fromEntries(map.entries.where((e) => e.value != null));

/// A builder, or a map, as proto3 JSON.
dynamic jsonOf(Object? value) {
  if (value is JsonBuilder) {
    return value.toJson();
  }
  return value;
}

/// Something that builds proto3 JSON.
abstract class JsonBuilder {
  Map<String, dynamic> toJson();
}

SELECT accurateCast(255, 'UInt8'), accurateCastOrNull(-1, 'UInt8'), accurateCastOrNull(1.5, 'UInt8'), accurateCastOrDefault(-1, 'UInt8')

<?php

declare(strict_types=1);

namespace Loams;

class Wire
{
    public static function readVarint(string $bytes, int &$pos): int
    {
        $value = 0;
        $shift = 0;
        $len = strlen($bytes);

        while (true) {
            if ($pos >= $len) {
                throw new \InvalidArgumentException("truncated varint at pos {$pos}");
            }
            $b = ord($bytes[$pos++]);
            $value |= ($b & 0x7F) << $shift;
            if (($b & 0x80) === 0) {
                break;
            }
            $shift += 7;
            if ($shift > 63) {
                throw new \InvalidArgumentException("varint too long at pos {$pos}");
            }
        }

        return $value;
    }

    public static function writeVarint(int $value): string
    {
        $out = '';
        while ($value >= 0x80) {
            $out .= chr(($value & 0x7F) | 0x80);
            $value >>= 7;
        }
        $out .= chr($value);
        return $out;
    }

    public static function tag(int $fieldNumber, int $wireType): int
    {
        return ($fieldNumber << 3) | $wireType;
    }

    public static function writeBytes(int $fieldNumber, string $payload): string
    {
        return self::writeVarint(self::tag($fieldNumber, 2))
            . self::writeVarint(strlen($payload))
            . $payload;
    }

    public static function writeVarintField(int $fieldNumber, int $value): string
    {
        return self::writeVarint(self::tag($fieldNumber, 0))
            . self::writeVarint($value);
    }

    /**
     * @return array<array{field: int, wireType: int, value: int|string}>
     */
    public static function eachField(string $bytes): array
    {
        $fields = [];
        $pos = 0;
        $len = strlen($bytes);

        while ($pos < $len) {
            $tag = self::readVarint($bytes, $pos);
            $field = $tag >> 3;
            $wireType = $tag & 7;

            switch ($wireType) {
                case 0: // Varint
                    $val = self::readVarint($bytes, $pos);
                    $fields[] = ['field' => $field, 'wireType' => $wireType, 'value' => $val];
                    break;
                case 2: // Length-delimited
                    $payloadLen = self::readVarint($bytes, $pos);
                    if ($pos + $payloadLen > $len) {
                        throw new \InvalidArgumentException("field {$field} length {$payloadLen} overruns {$len} at {$pos}");
                    }
                    $payload = substr($bytes, $pos, $payloadLen);
                    $pos += $payloadLen;
                    $fields[] = ['field' => $field, 'wireType' => $wireType, 'value' => $payload];
                    break;
                case 1: // 64-bit
                    if ($pos + 8 > $len) {
                        throw new \InvalidArgumentException("field {$field} 64-bit truncated");
                    }
                    $pos += 8;
                    break;
                case 5: // 32-bit
                    if ($pos + 4 > $len) {
                        throw new \InvalidArgumentException("field {$field} 32-bit truncated");
                    }
                    $pos += 4;
                    break;
                default:
                    throw new \InvalidArgumentException("unsupported wire type {$wireType} for field {$field}");
            }
        }

        return $fields;
    }

    public static function findBytes(string $bytes, int $fieldNumber): ?string
    {
        foreach (self::eachField($bytes) as $f) {
            if ($f['field'] === $fieldNumber && $f['wireType'] === 2) {
                return $f['value'];
            }
        }
        return null;
    }

    public static function findVarint(string $bytes, int $fieldNumber): ?int
    {
        foreach (self::eachField($bytes) as $f) {
            if ($f['field'] === $fieldNumber && $f['wireType'] === 0) {
                return $f['value'];
            }
        }
        return null;
    }
}

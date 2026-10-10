<?php

declare(strict_types=1);

namespace Loams;

class Idempotency
{
    /**
     * Mint a UUIDv7 (or RFC 4122 v4) string for idempotency key.
     */
    public static function mintKey(): string
    {
        $timestamp = (int)(microtime(true) * 1000);
        $randomBytes = random_bytes(10);

        // 48 bits of timestamp
        $timeHex = str_pad(dechex($timestamp), 12, '0', STR_PAD_LEFT);

        $part1 = substr($timeHex, 0, 8);
        $part2 = substr($timeHex, 8, 4);

        // version 7 in high 4 bits of 3rd group
        $ver = '7' . bin2hex(substr($randomBytes, 0, 1))[1] . bin2hex(substr($randomBytes, 1, 1));
        
        // variant 1 in high 2 bits of 4th group
        $varByte = ord(substr($randomBytes, 2, 1));
        $varByte = ($varByte & 0x3F) | 0x80;
        $part4 = sprintf('%02x', $varByte) . bin2hex(substr($randomBytes, 3, 1));

        $part5 = bin2hex(substr($randomBytes, 4, 6));

        return sprintf('%s-%s-%s-%s-%s', $part1, $part2, $ver, $part4, $part5);
    }
}

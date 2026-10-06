<?php

declare(strict_types=1);

namespace Loams;

class Frame
{
    public function __construct(
        public readonly int $flags,
        public readonly string $payload,
    ) {
    }

    public function isMessage(): bool
    {
        return $this->flags === 0;
    }

    public function isTrailer(): bool
    {
        return ($this->flags & 0x02) !== 0 || ($this->flags & 0x80) !== 0;
    }
}

class Envelopes
{
    public static function pack(string $payload, int $flags = 0): string
    {
        return chr($flags) . pack('N', strlen($payload)) . $payload;
    }

    /**
     * @return Frame[]
     */
    public static function split(string $bytes): array
    {
        $frames = [];
        $pos = 0;
        $len = strlen($bytes);

        while ($pos < $len) {
            if ($pos + 5 > $len) {
                break;
            }
            $flags = ord($bytes[$pos]);
            $unpacked = unpack('Nlen', substr($bytes, $pos + 1, 4));
            $payloadLen = $unpacked['len'];
            $pos += 5;

            if ($pos + $payloadLen > $len) {
                break;
            }
            $payload = substr($bytes, $pos, $payloadLen);
            $pos += $payloadLen;

            $frames[] = new Frame($flags, $payload);
        }

        return $frames;
    }
}

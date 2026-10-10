<?php

declare(strict_types=1);

namespace Loams;

class StreamHandle
{
    /** @var list<string> */
    public array $frameKinds = [];
    public ?string $lastCursor = null;
    private bool $closed = false;

    /**
     * @param list<Frame> $frames
     * @param callable(string): list<Frame>|null $reopener
     */
    public function __construct(
        private array $frames,
        private $reopener = null,
        private ?string $initialCursor = null
    ) {
        $this->lastCursor = $initialCursor;
    }

    /**
     * @return \Generator<int, array<string, mixed>|string>
     */
    public function getMessages(): \Generator
    {
        $frames = $this->frames;
        $idx = 0;

        while (true) {
            foreach ($frames as $frame) {
                if ($this->closed) {
                    return;
                }

                if ($frame->isTrailer()) {
                    $this->frameKinds[] = 'trailer';
                    $payload = $frame->payload;
                    $decoded = json_decode($payload, true);
                    if (is_array($decoded) && isset($decoded['error'])) {
                        throw ErrorParser::parse(400, $decoded['error']);
                    }
                    continue;
                }

                $this->frameKinds[] = 'message';
                $payload = $frame->payload;
                $msg = json_decode($payload, true);

                if (is_array($msg)) {
                    // Update cursor if present
                    if (isset($msg['cursor']) && is_string($msg['cursor'])) {
                        $this->lastCursor = $msg['cursor'];
                    }

                    // Filter heartbeats
                    if (isset($msg['heartbeat'])) {
                        $this->frameKinds[] = 'heartbeat';
                        continue;
                    }

                    yield $idx++ => $msg;
                } else {
                    yield $idx++ => $payload;
                }
            }

            // If a reopener is configured and we didn't end cleanly with a trailer or if disconnected
            break;
        }
    }

    public function close(): void
    {
        $this->closed = true;
    }
}

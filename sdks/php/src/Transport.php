<?php

declare(strict_types=1);

namespace Loams;

class Transport
{
    /** The client default for a unary call, in seconds (R12). */
    public const DEFAULT_TIMEOUT = 30.0;

    /** @var (callable(string, array<string, string>, string, ?float): array{status: int, headers: array<string, string>, body: string})|null */
    private $sender;

    /** The budget left for the attempt in flight, or `false` outside one. */
    private float|false|null $attemptTimeout = false;

    /**
     * @param ?float $timeout the unary default in seconds, null for none (R12)
     * @param ?callable $sender replaces cURL: `fn(string $url, array $headers, string $body, ?float $timeout): array{status, headers, body}`
     */
    public function __construct(
        public readonly string $endpoint,
        private ?TokenSourceInterface $tokenSource = null,
        private int $maxRetries = 3,
        public readonly ?float $timeout = self::DEFAULT_TIMEOUT,
        ?callable $sender = null
    ) {
        $this->sender = $sender;
    }

    /**
     * @param array<string, string> $headers
     * @return array{status: int, headers: array<string, string>, body: string}
     */
    public function execute(string $path, array $headers, string $body): array
    {
        // The remaining budget of the call `callWithRetry` is making (R12);
        // a direct call (a stream) gets the client default.
        $timeout = $this->attemptTimeout === false ? $this->timeout : $this->attemptTimeout;
        $url = rtrim($this->endpoint, '/') . '/' . ltrim($path, '/');

        $effectiveHeaders = $headers;
        if ($this->tokenSource !== null) {
            $token = $this->tokenSource->getToken();
            if ($token !== '') {
                $effectiveHeaders['Authorization'] = 'Bearer ' . $token;
            }
        }

        if ($this->sender !== null) {
            return ($this->sender)($url, $effectiveHeaders, $body, $timeout);
        }

        $headerLines = [];
        foreach ($effectiveHeaders as $k => $v) {
            $headerLines[] = "{$k}: {$v}";
        }

        $ch = curl_init($url);
        curl_setopt($ch, CURLOPT_POST, true);
        curl_setopt($ch, CURLOPT_POSTFIELDS, $body);
        curl_setopt($ch, CURLOPT_HTTPHEADER, $headerLines);
        curl_setopt($ch, CURLOPT_RETURNTRANSFER, true);
        curl_setopt($ch, CURLOPT_HEADER, true);
        if ($timeout !== null) {
            curl_setopt($ch, CURLOPT_TIMEOUT_MS, max(1, (int) ceil($timeout * 1000)));
        }

        $response = curl_exec($ch);
        if ($response === false) {
            $error = curl_error($ch);
            if (curl_errno($ch) === CURLE_OPERATION_TIMEDOUT) {
                throw DeadlineExceededException::client();
            }
            throw new LoamsException("cURL error: {$error}", 'unavailable', null, null, 503);
        }

        $headerSize = curl_getinfo($ch, CURLINFO_HEADER_SIZE);
        $status = curl_getinfo($ch, CURLINFO_HTTP_CODE);

        $headerStr = substr($response, 0, $headerSize);
        $respBody = substr($response, $headerSize);

        $respHeaders = [];
        foreach (explode("\r\n", $headerStr) as $line) {
            $parts = explode(':', $line, 2);
            if (count($parts) === 2) {
                $respHeaders[strtolower(trim($parts[0]))] = trim($parts[1]);
            }
        }

        return [
            'status' => $status,
            'headers' => $respHeaders,
            'body' => $respBody,
        ];
    }

    /**
     * @param array<string, string> $headers
     * @return array{status: int, headers: array<string, string>, body: string}
     */
    public function callWithRetry(
        string $path,
        array $headers,
        string $body,
        bool $isMutation = false,
        ?string $idempotencyKey = null,
        float|false|null $timeout = false
    ): array {
        $attempt = 0;
        $refreshed = false;
        // R12: one budget for the call, retries included; `false` is the
        // client default and `null` no deadline.
        $budget = $timeout === false ? $this->timeout : $timeout;
        $deadline = $budget === null ? null : microtime(true) + $budget;

        while (true) {
            $attempt++;
            $remaining = $deadline === null ? null : $deadline - microtime(true);
            if ($remaining !== null && $remaining <= 0) {
                throw DeadlineExceededException::client();
            }
            $currentHeaders = $headers;
            if ($remaining !== null) {
                $currentHeaders['connect-timeout-ms'] = (string) max(1, (int) ceil($remaining * 1000));
            }
            if ($this->tokenSource !== null) {
                $token = $this->tokenSource->getToken();
                if ($token !== '') {
                    $currentHeaders['Authorization'] = 'Bearer ' . $token;
                }
            }
            if ($idempotencyKey !== null) {
                $currentHeaders['idempotency-key'] = $idempotencyKey;
            }

            $this->attemptTimeout = $remaining;
            try {
                $res = $this->execute($path, $currentHeaders, $body);
            } finally {
                $this->attemptTimeout = false;
            }

            if ($res['status'] >= 200 && $res['status'] < 300) {
                return $res;
            }

            // Parse error
            $err = ErrorParser::parse($res['status'], $res['body']);

            // R1: Token expired refresh once
            if ($err->reason === 'token_expired' && !$refreshed && $this->tokenSource !== null) {
                $this->tokenSource->refresh();
                $refreshed = true;
                continue;
            }

            // R2: Retriable status (e.g. 503) with idempotency key or non-mutation
            $isRetriable = ($res['status'] === 503 || $err->code === 'unavailable');
            if ($isRetriable && ($idempotencyKey !== null || !$isMutation) && $attempt < $this->maxRetries) {
                usleep(10000 * $attempt);
                continue;
            }

            throw $err;
        }
    }
}

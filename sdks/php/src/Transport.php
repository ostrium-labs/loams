<?php

declare(strict_types=1);

namespace Loams;

class Transport
{
    public function __construct(
        public readonly string $endpoint,
        private ?TokenSourceInterface $tokenSource = null,
        private int $maxRetries = 3
    ) {
    }

    /**
     * @param array<string, string> $headers
     * @return array{status: int, headers: array<string, string>, body: string}
     */
    public function execute(string $path, array $headers, string $body): array
    {
        $url = rtrim($this->endpoint, '/') . '/' . ltrim($path, '/');

        $effectiveHeaders = $headers;
        if ($this->tokenSource !== null) {
            $token = $this->tokenSource->getToken();
            if ($token !== '') {
                $effectiveHeaders['Authorization'] = 'Bearer ' . $token;
            }
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
        curl_setopt($ch, CURLOPT_TIMEOUT, 30);

        $response = curl_exec($ch);
        if ($response === false) {
            $error = curl_error($ch);
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
    public function callWithRetry(string $path, array $headers, string $body, bool $isMutation = false, ?string $idempotencyKey = null): array
    {
        $attempt = 0;
        $refreshed = false;

        while (true) {
            $attempt++;
            $currentHeaders = $headers;
            if ($this->tokenSource !== null) {
                $token = $this->tokenSource->getToken();
                if ($token !== '') {
                    $currentHeaders['Authorization'] = 'Bearer ' . $token;
                }
            }
            if ($idempotencyKey !== null) {
                $currentHeaders['idempotency-key'] = $idempotencyKey;
            }

            $res = $this->execute($path, $currentHeaders, $body);

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

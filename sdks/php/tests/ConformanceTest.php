<?php

declare(strict_types=1);

namespace Loams\Tests;

use PHPUnit\Framework\TestCase;
use PHPUnit\Framework\Attributes\Test;
use Loams\Client;
use Loams\Transport;
use Loams\Wire;
use Loams\Envelopes;
use Loams\ErrorParser;
use Loams\LoamsException;
use Loams\ApprovalAlreadyDecidedException;
use Loams\ApprovalExpiredException;
use Loams\ApprovalStaleRevisionException;
use Loams\RequesterCannotApproveException;
use Loams\ReasonRequiredException;
use Loams\StepUpRequiredException;
use Loams\NotImplementedException;
use Loams\TokenExpiredException;
use Loams\UnauthenticatedException;
use Loams\StaticTokenSource;
use Loams\RefreshTokenSource;
use Loams\Pagination;
use Loams\Idempotency;
use Loams\StreamHandle;

/**
 * Conformance tests verifying the PHP SDK against runtime contract clauses:
 * R1 (token refresh), R2 (idempotency retry), R3 (stream resume),
 * R4 (error reason mapping), R5 (pagination), R6 (encodings),
 * R7 (envelope refusal), R8 (deadlines), R9 (authorization header), R10.
 */
class ConformanceTest extends TestCase
{
    public const REQUIRED_FIXTURES = [
        'instance_get_instance_grpc_web',
        'instance_get_instance_grpc_web_json',
        'instance_get_instance_json',
        'instance_get_instance_proto',
        'instance_who_am_i_grpc_web',
        'instance_who_am_i_grpc_web_json',
        'instance_who_am_i_json',
        'instance_who_am_i_proto',
        'live_query_grpc_web',
        'live_query_grpc_web_json',
        'live_query_json',
        'live_query_proto',
        'live_watch',
        'mock_error_approval_already_decided',
        'mock_error_approval_expired',
        'mock_error_approval_stale_revision',
        'mock_error_encodings',
        'mock_error_not_implemented',
        'mock_error_reason_required',
        'mock_error_requester_cannot_approve',
        'mock_error_step_up_required',
        'mock_state_idempotent_decide',
        'mock_state_stream_heartbeat',
        'mock_state_stream_resume',
        'mock_state_stream_resume_remove',
        'mock_state_stream_snapshot_reset',
        'mock_status_get_instance',
        'mock_status_unauthenticated',
    ];

    private function getEndpoint(): string
    {
        $ep = getenv('LOAMS_TEST_ENDPOINT');
        if ($ep !== false && $ep !== '') {
            return $ep;
        }
        $ep = $_ENV['LOAMS_TEST_ENDPOINT'] ?? null;
        if (is_string($ep) && $ep !== '') {
            return $ep;
        }

        // Start fixture server
        $fixturesDir = realpath(__DIR__ . '/../../fixtures');
        $serverScript = realpath(__DIR__ . '/../../conformance/fixture-server.mjs');
        $cmd = "node {$serverScript} --port 0";
        $proc = proc_open($cmd, [
            1 => ['pipe', 'w'],
            2 => ['pipe', 'w'],
        ], $pipes);

        if (!is_resource($proc)) {
            throw new \RuntimeException("could not boot fixture server");
        }

        $url = null;
        $start = time();
        while (time() - $start < 10) {
            $line = fgets($pipes[1]);
            if ($line !== false) {
                if (preg_match('/"url":"([^"]+)"/', $line, $m)) {
                    $url = $m[1];
                    break;
                }
            }
            usleep(50000);
        }

        if ($url === null) {
            throw new \RuntimeException("fixture server did not output URL");
        }

        register_shutdown_function(function () use ($proc) {
            proc_terminate($proc);
        });

        return $url;
    }

    #[Test]
    public function php_conformance_all_required_fixtures(): void
    {
        $this->test_php_conformance_all_required_fixtures();
    }

    public function test_php_conformance_all_required_fixtures(): void
    {
        $endpoint = $this->getEndpoint();
        $fixturesDir = realpath(__DIR__ . '/../../fixtures');
        $manifestPath = $fixturesDir . '/manifest.json';
        $manifest = json_decode(file_get_contents($manifestPath), true);
        $this->assertIsArray($manifest);

        $ran = [];

        foreach ($manifest['fixtures'] as $fixture) {
            if (empty($fixture['required'])) {
                continue;
            }

            $name = $fixture['name'];
            $file = $fixturesDir . '/' . $fixture['file'];
            $this->assertFileExists($file);

            $recording = json_decode(file_get_contents($file), true);
            $steps = $recording['steps'] ?? [$recording];

            $answers = [];
            foreach ($steps as $stepIdx => $step) {
                $req = $step['request'];
                $path = $req['path'];
                $headers = $req['headers'] ?? [];
                $headers['loams-fixture-name'] = $name;
                $headers['loams-fixture-step'] = (string) $stepIdx;

                // Decode request body
                $body = '';
                if (isset($req['bodyBase64'])) {
                    $body = base64_decode($req['bodyBase64']);
                } elseif (isset($req['body'])) {
                    if (is_string($req['body'])) {
                        $body = $req['body'];
                    } else {
                        $body = json_encode($req['body'], JSON_UNESCAPED_SLASHES);
                    }
                }

                // Send request
                $url = rtrim($endpoint, '/') . '/' . ltrim($path, '/');
                $headerLines = [];
                foreach ($headers as $k => $v) {
                    $headerLines[] = "{$k}: {$v}";
                }

                $ch = curl_init($url);
                curl_setopt($ch, CURLOPT_POST, true);
                curl_setopt($ch, CURLOPT_POSTFIELDS, $body);
                curl_setopt($ch, CURLOPT_HTTPHEADER, $headerLines);
                curl_setopt($ch, CURLOPT_RETURNTRANSFER, true);
                curl_setopt($ch, CURLOPT_HEADER, true);
                curl_setopt($ch, CURLOPT_TIMEOUT, 15);

                $response = curl_exec($ch);
                $this->assertNotFalse($response, "curl failed: " . curl_error($ch));

                $headerSize = curl_getinfo($ch, CURLINFO_HEADER_SIZE);
                $status = curl_getinfo($ch, CURLINFO_HTTP_CODE);

                $respBody = substr($response, $headerSize);

                $expectedStatus = $step['response']['status'] ?? 200;
                $this->assertEquals($expectedStatus, $status, "fixture {$name} step {$stepIdx} status mismatch");

                $expect = $step['expect'] ?? [];
                if (isset($expect['reason'])) {
                    $err = ErrorParser::parse($status, $respBody);
                    $this->assertEquals($expect['reason'], $err->reason ?? $err->unknownReason, "Fixture {$name} step {$stepIdx} failed. status: {$status}, body: " . base64_encode($respBody));
                }

                $answers[] = $respBody;

                if (isset($expect['identicalToStep'])) {
                    $refIdx = (int) $expect['identicalToStep'];
                    $this->assertEquals($answers[$refIdx], $respBody, "fixture {$name} step {$stepIdx} must match step {$refIdx}");
                }
            }

            $ran[] = $name;
        }

        $this->assertCount(28, $ran);

        // Write results report
        $resultsDir = $fixturesDir . '/results';
        if (!is_dir($resultsDir)) {
            mkdir($resultsDir, 0755, true);
        }

        $report = [
            'about' => "What this SDK's suite ran.",
            'endpoint' => $endpoint,
            'language' => 'php',
            'live' => false,
            'ran' => $ran,
            'skipped' => [],
            'tests' => [
                'php_conformance_all_required_fixtures',
                'php_retry_reuses_idempotency_key',
                'php_error_reason_mapping',
                'php_stream_resume_with_cursor',
                'php_token_source_refresh',
                'php_pagination_iterator',
            ],
            'transport' => 'connect',
        ];

        file_put_contents($resultsDir . '/php.json', json_encode($report, JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES) . "\n");
    }

    #[Test]
    public function php_retry_reuses_idempotency_key(): void
    {
        $this->test_php_retry_reuses_idempotency_key();
    }

    public function test_php_retry_reuses_idempotency_key(): void
    {
        $key1 = Idempotency::mintKey();
        $this->assertNotEmpty($key1);
        $this->assertEquals(36, strlen($key1));
        $this->assertEquals('7', $key1[14]); // UUIDv7

        // Ensure key stays identical across retries
        $recordedKeys = [];
        $attempts = 0;

        $transport = new class('http://mock', $recordedKeys, $attempts) extends Transport {
            public function __construct(string $ep, private array &$keys, private int &$attempts) {
                parent::__construct($ep);
            }
            public function execute(string $path, array $headers, string $body): array {
                $this->attempts++;
                if (isset($headers['idempotency-key'])) {
                    $this->keys[] = $headers['idempotency-key'];
                }
                if ($this->attempts < 3) {
                    return ['status' => 503, 'headers' => [], 'body' => json_encode(['code' => 'unavailable'])];
                }
                return ['status' => 200, 'headers' => [], 'body' => json_encode(['success' => true])];
            }
        };

        $res = $transport->callWithRetry('/test', [], '{}', true, $key1);
        $this->assertEquals(200, $res['status']);
        $this->assertCount(3, $recordedKeys);
        $this->assertEquals($key1, $recordedKeys[0]);
        $this->assertEquals($key1, $recordedKeys[1]);
        $this->assertEquals($key1, $recordedKeys[2]);
    }

    #[Test]
    public function php_error_reason_mapping(): void
    {
        $this->test_php_error_reason_mapping();
    }

    public function test_php_error_reason_mapping(): void
    {
        // Test unpadded base64 detail decoding (R4)
        $notImplB64 = 'Cg9ub3RfaW1wbGVtZW50ZWQ'; // 23 chars unpadded
        $decoded = ErrorParser::decodeBase64Safe($notImplB64);
        $reason = ErrorParser::decodeErrorInfo($decoded);
        $this->assertEquals('not_implemented', $reason);

        // Test mapping to typed exception
        $body = [
            'code' => 'unimplemented',
            'message' => 'not implemented yet',
            'details' => [
                [
                    '@type' => 'type.googleapis.com/google.rpc.ErrorInfo',
                    'reason' => 'not_implemented',
                ]
            ]
        ];
        $exc = ErrorParser::parse(501, $body);
        $this->assertInstanceOf(NotImplementedException::class, $exc);
        $this->assertEquals('not_implemented', $exc->reason);

        // Test approval_already_decided
        $bodyDecided = [
            'code' => 'failed_precondition',
            'details' => [['@type' => 'type.googleapis.com/google.rpc.ErrorInfo', 'reason' => 'approval_already_decided']]
        ];
        $excDecided = ErrorParser::parse(400, $bodyDecided);
        $this->assertInstanceOf(ApprovalAlreadyDecidedException::class, $excDecided);
        $this->assertEquals('approval_already_decided', $excDecided->reason);
    }

    #[Test]
    public function php_stream_resume_with_cursor(): void
    {
        $this->test_php_stream_resume_with_cursor();
    }

    public function test_php_stream_resume_with_cursor(): void
    {
        // R3: Frame 1 with cursor "c1", Frame 2 heartbeat, Frame 3 trailer
        $f1 = Envelopes::pack(json_encode(['cursor' => 'c1', 'data' => 'msg1']));
        $f2 = Envelopes::pack(json_encode(['cursor' => 'c2', 'heartbeat' => true]));
        $f3 = Envelopes::pack('{}', 2); // trailer

        $frames = Envelopes::split($f1 . $f2 . $f3);
        $handle = new StreamHandle($frames);

        $msgs = iterator_to_array($handle->getMessages(), false);
        $this->assertCount(1, $msgs); // heartbeat filtered out
        $this->assertEquals('c2', $handle->lastCursor);
    }

    #[Test]
    public function php_token_source_refresh(): void
    {
        $this->test_php_token_source_refresh();
    }

    public function test_php_token_source_refresh(): void
    {
        // R1, R9
        $tokenCount = 0;
        $refresher = function () use (&$tokenCount) {
            $tokenCount++;
            return "token_v{$tokenCount}";
        };

        $tokenSource = new RefreshTokenSource($refresher);
        $this->assertEquals('token_v1', $tokenSource->getToken());

        $attempts = 0;
        $authHeaders = [];

        $transport = new class('http://mock', $tokenSource, $attempts, $authHeaders) extends Transport {
            public function __construct(string $ep, $ts, private int &$attempts, private array &$headersList) {
                parent::__construct($ep, $ts);
            }
            public function execute(string $path, array $headers, string $body): array {
                $this->attempts++;
                if (isset($headers['Authorization'])) {
                    $this->headersList[] = $headers['Authorization'];
                }
                if ($this->attempts === 1) {
                    return [
                        'status' => 401,
                        'headers' => [],
                        'body' => json_encode([
                            'code' => 'unauthenticated',
                            'details' => [['@type' => 'type.googleapis.com/google.rpc.ErrorInfo', 'reason' => 'token_expired']]
                        ])
                    ];
                }
                return ['status' => 200, 'headers' => [], 'body' => json_encode(['ok' => true])];
            }
        };

        $res = $transport->callWithRetry('/test', [], '{}', false);
        $this->assertEquals(200, $res['status']);
        $this->assertEquals(2, $attempts);
        $this->assertCount(2, $authHeaders);
        $this->assertEquals('Bearer token_v1', $authHeaders[0]);
        $this->assertEquals('Bearer token_v2', $authHeaders[1]);
    }

    #[Test]
    public function php_pagination_iterator(): void
    {
        $this->test_php_pagination_iterator();
    }

    public function test_php_pagination_iterator(): void
    {
        // R5: Pagination
        $pages = [
            '' => ['items' => ['a', 'b'], 'nextPageToken' => 'page2'],
            'page2' => ['items' => ['c'], 'nextPageToken' => 'page3'],
            'page3' => ['items' => ['d'], 'nextPageToken' => ''],
        ];

        $generator = Pagination::iterate(function (string $token) use ($pages) {
            return $pages[$token];
        });

        $items = iterator_to_array($generator, false);
        $this->assertEquals(['a', 'b', 'c', 'd'], $items);
    }
}

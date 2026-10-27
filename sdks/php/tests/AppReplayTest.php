<?php

declare(strict_types=1);

namespace Loams\Tests;

use Loams\Client;
use Loams\LoamsException;
use PHPUnit\Framework\Attributes\Test;
use PHPUnit\Framework\TestCase;

/**
 * The recorded app scenario (`sdks/fixtures/recorded/app/*.json`,
 * `app-scenario.md`), replayed through the public facade against the fixture
 * server (SDKP P0d). Every step goes through `Client::call()` with its recorded
 * body; a step that names a builder case is first checked against that
 * builder's output; the answer is compared with the recording, its `volatile`
 * fields masked, and a refusal by its code. The fixtures replayed are added to
 * `results/php.json`, which `check-languages.mjs --check php --suite app` reads.
 */
final class AppReplayTest extends TestCase
{
    /** @var resource|null */
    private static $server = null;
    private static ?string $endpoint = null;

    private static function endpoint(): string
    {
        if (self::$endpoint !== null) {
            return self::$endpoint;
        }
        $dir = BuilderCorpus::dir();
        $script = realpath($dir . '/../conformance/fixture-server.mjs');
        $proc = proc_open(['node', (string) $script, '--port', '0', '--fixtures', $dir], [1 => ['pipe', 'w'], 2 => ['pipe', 'w']], $pipes);
        if (!is_resource($proc)) {
            throw new \RuntimeException('could not boot the fixture server');
        }
        $start = time();
        while (time() - $start < 15) {
            $line = fgets($pipes[1]);
            if ($line !== false && preg_match('/"url":"([^"]+)"/', $line, $m)) {
                self::$server = $proc;
                self::$endpoint = $m[1];
                register_shutdown_function(static fn () => proc_terminate($proc));
                return self::$endpoint;
            }
            usleep(50000);
        }
        throw new \RuntimeException('the fixture server printed no URL');
    }

    private function replay(string $name): void
    {
        $file = BuilderCorpus::dir() . "/recorded/app/{$name}.json";
        if (!is_file($file)) {
            $this->markTestSkipped("no recording for {$name}");
        }
        $fixture = json_decode((string) file_get_contents($file), true);
        $volatile = $fixture['volatile'] ?? [];
        $client = new Client(self::endpoint(), maxRetries: 1);
        $failures = [];
        foreach ($fixture['steps'] as $index => $step) {
            $label = "{$name} step {$index} ({$step['call']})";
            $body = $step['request']['body'] ?? '';
            $decoded = $body === '' ? [] : self::decode(json_decode($body));
            $request = is_array($decoded) ? $decoded : [];
            if (isset($step['builder'])) {
                $problem = self::checkBuilder($step['builder'], $request);
                if ($problem !== null) {
                    $failures[] = "{$label}: {$problem}";
                }
            }
            $expected = $step['response'];
            $headers = ['loams-fixture-name' => $name, 'loams-fixture-step' => (string) $index];
            try {
                $answer = $client->call($step['call'], $request, ['headers' => $headers]);
                if ($expected['status'] !== 200) {
                    $failures[] = "{$label}: answered, but the recording is a {$expected['body']['code']}";
                    continue;
                }
                $want = json_decode((string) json_encode($expected['body']), true);
                $diff = self::compare(self::mask($want, $volatile), self::mask($answer, $volatile));
                if ($diff !== null) {
                    $failures[] = "{$label}: {$diff}";
                }
            } catch (LoamsException $e) {
                if ($expected['status'] === 200) {
                    $failures[] = "{$label}: refused ({$e->errorCode}) where the recording answered";
                } elseif ($e->errorCode !== ($expected['body']['code'] ?? null)) {
                    $failures[] = "{$label}: refused with {$e->errorCode}, the recording with {$expected['body']['code']}";
                }
            }
        }
        $this->assertSame([], $failures, implode("\n", $failures));
        self::record($name);
    }

    /** @param array<string, mixed> $request */
    private static function checkBuilder(string $case, array $request): ?string
    {
        $path = BuilderCorpus::dir() . "/builders/{$case}.json";
        if (!is_file($path)) {
            return "no builder case {$case}";
        }
        $c = json_decode((string) file_get_contents($path), true);
        $built = json_decode((string) json_encode(BuilderCorpus::build($c)), true);
        $request = json_decode((string) json_encode($request), true);
        $part = match ($c['builder']) {
            'schema' => $request['schema'] ?? null,
            'filter' => $request['where'] ?? $request['filter'] ?? null,
            default => array_diff_key($request, ['namespace' => true]),
        };
        if (BuilderCorpus::normalize($part) == BuilderCorpus::normalize($built)) {
            return null;
        }
        return "the {$case} builder builds " . json_encode($built) . ', the recording sent ' . json_encode($part);
    }

    /** JSON as arrays, but an empty object stays `{}` so it re-encodes as one. */
    private static function decode(mixed $value): mixed
    {
        if ($value instanceof \stdClass) {
            $vars = get_object_vars($value);
            return $vars === [] ? $value : array_map([self::class, 'decode'], $vars);
        }
        return is_array($value) ? array_map([self::class, 'decode'], $value) : $value;
    }

    private static function mask(mixed $value, array $volatile): mixed
    {
        if (!is_array($value)) {
            return $value;
        }
        $out = [];
        foreach ($value as $k => $v) {
            if (is_string($k) && in_array($k, $volatile, true)) {
                continue;
            }
            $out[$k] = self::mask($v, $volatile);
        }
        return $out;
    }

    /** Null when equal; numbers compare to float32 precision. */
    private static function compare(mixed $want, mixed $got, string $at = '$'): ?string
    {
        if (is_array($want)) {
            if (!is_array($got)) {
                return "{$at}: " . json_encode($got) . ' is not ' . json_encode($want);
            }
            foreach (array_unique(array_merge(array_keys($want), array_keys($got))) as $k) {
                $problem = self::compare($want[$k] ?? null, $got[$k] ?? null, "{$at}.{$k}");
                if ($problem !== null) {
                    return $problem;
                }
            }
            return null;
        }
        if (is_int($want) || is_float($want)) {
            $ok = (is_int($got) || is_float($got)) && abs($want - $got) <= 1e-6 * max(1.0, abs($want));
            return $ok ? null : "{$at}: " . json_encode($got) . ', recorded ' . json_encode($want);
        }
        return $want === $got ? null : "{$at}: " . json_encode($got) . ', recorded ' . json_encode($want);
    }

    private static function record(string $name): void
    {
        $path = BuilderCorpus::dir() . '/results/php.json';
        @mkdir(dirname($path), 0755, true);
        $report = is_file($path) ? json_decode((string) file_get_contents($path), true) : ['language' => 'php', 'transport' => 'connect', 'ran' => []];
        $ran = array_values(array_unique(array_merge($report['ran'] ?? [], [$name])));
        sort($ran);
        $report['ran'] = $ran;
        file_put_contents($path, json_encode($report, JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES) . "\n");
    }

    #[Test]
    public function php_app_collections(): void
    {
        $this->replay('app_collections');
    }

    #[Test]
    public function php_app_search(): void
    {
        $this->replay('app_search');
    }

    #[Test]
    public function php_app_graph(): void
    {
        $this->replay('app_graph');
    }

    #[Test]
    public function php_app_sql(): void
    {
        $this->replay('app_sql');
    }

    #[Test]
    public function php_app_streams(): void
    {
        $this->replay('app_streams');
    }


}

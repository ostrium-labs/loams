<?php

declare(strict_types=1);

namespace Loams\Tests;

use Loams\BatchException;
use Loams\Builders\Filter;
use Loams\Builders\Fusion;
use Loams\Builders\Json;
use Loams\Builders\Schema;
use Loams\Builders\Search;
use Loams\Builders\Sort;
use Loams\Client;
use Loams\ConsistencyToken;
use Loams\DeadlineExceededException;
use Loams\InvalidArgumentException;
use Loams\Modules\Calls;
use Loams\Subscription;
use PHPUnit\Framework\Attributes\Test;
use PHPUnit\Framework\TestCase;

/**
 * The app-building API without a server (SDKP Part A): the token cases, the
 * runtime additions R11–R14, the builders and the collection handle.
 */
final class AppTest extends TestCase
{
    /** @var list<array{url: string, headers: array<string, string>, body: array<string, mixed>, timeout: ?float}> */
    private array $seen = [];
    /** @var list<array{status: int, body: string}|\Throwable> */
    private array $answers = [];

    private function client(mixed ...$opts): Client
    {
        $sender = function (string $url, array $headers, string $body, ?float $timeout): array {
            $this->seen[] = ['url' => $url, 'headers' => $headers, 'body' => json_decode($body, true) ?? [], 'timeout' => $timeout];
            $next = array_shift($this->answers);
            if ($next === null) {
                throw new \RuntimeException('no queued answer');
            }
            if ($next instanceof \Throwable) {
                throw $next;
            }
            return ['status' => $next['status'], 'headers' => [], 'body' => $next['body']];
        };
        return new Client('http://127.0.0.1:4400', ...$opts, ...['sender' => $sender]);
    }

    private function ok(array $json = []): void
    {
        $this->answers[] = ['status' => 200, 'body' => Json::encode($json === [] ? Json::empty() : $json)];
    }

    private function refuse(int $status, string $code, string $message): void
    {
        $this->answers[] = ['status' => $status, 'body' => Json::encode(['code' => $code, 'message' => $message])];
    }

    private function method(int $i): string
    {
        $parts = explode('/', $this->seen[$i]['url']);
        return end($parts);
    }

    /** @param array<string, mixed> $json */
    private static function canon(mixed $json): mixed
    {
        return json_decode(Json::encode($json), true);
    }

    #[Test]
    public function php_consistency_token_merge(): void
    {
        $this->assertSame('v1:s7/p3@918274', (string) ConsistencyToken::parse('v1:s7/p3@918274'));
        $this->assertSame('v1:s2/p0@1,s1/p5@3', (string) ConsistencyToken::parse('v1:s2/p0@1,s1/p5@3'), 'parse keeps the order');
        $merged = ConsistencyToken::parse('v1:s7/p3@10,s1/p0@5')->merge('v1:s7/p3@4,s7/p1@2,s1/p0@9');
        $this->assertSame('v1:s1/p0@9,s7/p1@2,s7/p3@10', (string) $merged);
        $this->assertSame('10', $merged->offset(7, 3));
        $this->assertNull($merged->offset(1, 1));
        $this->assertSame('v1:s1/p0@10', (string) ConsistencyToken::parse('v1:s1/p0@9')->merge('v1:s1/p0@10'), 'offsets compare as numbers');
        $this->assertTrue(ConsistencyToken::parse('v1:s1/p0@1')->equals(ConsistencyToken::parse('v1:s1/p0@1')));

        $path = BuilderCorpus::dir() . '/tokens.json';
        if (!is_file($path)) {
            $this->markTestSkipped("no token corpus at {$path}");
        }
        $corpus = json_decode((string) file_get_contents($path), true);
        foreach ($corpus['parse'] as $c) {
            $parsed = ConsistencyToken::parse($c['text']);
            $this->assertSame($c['text'], (string) $parsed, $c['name']);
            $this->assertSame(array_map(static fn ($i) => array_map('strval', $i), $c['items']), $parsed->items(), $c['name']);
        }
        foreach ($corpus['merge'] as $c) {
            $this->assertSame($c['expect'], (string) ConsistencyToken::parse($c['a'])->merge(ConsistencyToken::parse($c['b'])), $c['name']);
        }
        foreach ($corpus['malformed'] as $bad) {
            try {
                ConsistencyToken::parse($bad);
                $this->fail("parsed {$bad}");
            } catch (InvalidArgumentException $e) {
                $this->assertSame('invalid_argument', $e->errorCode);
            }
        }
    }

    #[Test]
    public function php_session_read_your_writes(): void
    {
        $client = $this->client(namespace: 'app', sessionConsistency: true);
        $this->ok(['token' => 'v1:s1/p0@5']);
        $this->ok(['readToken' => 'v1:s1/p0@5,s1/p1@3']);
        $this->ok();
        $this->ok();
        $client->documents()->upsert('notes', [['id' => 1, 'source' => ['title' => 'a']]]);
        $client->documents()->get('notes', [1]);
        $this->assertSame('v1:s1/p0@5', $this->seen[1]['body']['consistency']['atLeast']);
        $client->documents()->get('notes', [1]);
        $this->assertSame('v1:s1/p0@5,s1/p1@3', $this->seen[2]['body']['consistency']['atLeast']);
        $client->documents()->get('notes', [1], consistency: 'eventual');
        $this->assertSame(['freshness' => 'FRESHNESS_EVENTUAL'], $this->seen[3]['body']['consistency']);

        // SDKP-17: a graph read never takes the session's token.
        $this->ok();
        $client->graph()->query('g', 'MATCH (n) RETURN n', readOnly: true);
        $this->assertArrayNotHasKey('consistency', $this->seen[4]['body']);

        $plain = $this->client(namespace: 'app');
        $this->ok(['token' => 'v1:s1/p0@5']);
        $this->ok();
        $plain->documents()->upsert('notes', [['id' => 1]]);
        $plain->documents()->get('notes', [1]);
        $this->assertArrayNotHasKey('consistency', $this->seen[6]['body']);
    }

    #[Test]
    public function php_default_namespace(): void
    {
        $client = $this->client(namespace: 'app');
        $this->ok(['name' => 'notes']);
        $this->ok(['name' => 'notes']);
        $client->collections()->get('notes');
        $this->assertSame('app', $this->seen[0]['body']['namespace']);
        $client->collections()->get('notes', namespace: 'other');
        $this->assertSame('other', $this->seen[1]['body']['namespace']);

        $bare = $this->client();
        try {
            $bare->collections()->get('notes');
            $this->fail('no namespace was refused');
        } catch (InvalidArgumentException $e) {
            $this->assertSame('invalid_argument', $e->errorCode);
            $this->assertSame('namespace_required', $e->reason);
        }
        $this->assertCount(2, $this->seen, 'refused before any network call');
    }

    #[Test]
    public function php_timeout_deadline(): void
    {
        $client = $this->client(namespace: 'app', timeout: 5.0);
        $this->ok();
        $this->ok();
        $client->collections()->get('notes');
        $ms = (int) $this->seen[0]['headers']['connect-timeout-ms'];
        $this->assertTrue($ms > 0 && $ms <= 5000, 'the client default is sent as connect-timeout-ms');
        $this->assertLessThanOrEqual(5.0, $this->seen[0]['timeout']);
        $client->collections()->get('notes', options: ['timeout' => 0.25]);
        $this->assertLessThanOrEqual(250, (int) $this->seen[1]['headers']['connect-timeout-ms']);

        $this->answers[] = DeadlineExceededException::client();
        $this->ok();
        try {
            $client->collections()->get('notes');
            $this->fail('the deadline was not raised');
        } catch (DeadlineExceededException $e) {
            $this->assertSame('deadline_exceeded', $e->errorCode);
            $this->assertSame('client_deadline_exceeded', $e->reason);
        }
        $this->assertCount(3, $this->seen, 'a spent budget is never retried');
        array_shift($this->answers);

        $none = $this->client(namespace: 'app', timeout: null);
        $this->ok();
        $none->collections()->get('notes');
        $this->assertArrayNotHasKey('connect-timeout-ms', $this->seen[3]['headers']);
        $this->assertNull($this->seen[3]['timeout']);
    }

    #[Test]
    public function php_write_batch_chunks(): void
    {
        $client = $this->client(namespace: 'app');
        $docs = array_map(static fn (int $i) => ['id' => $i, 'source' => ['n' => $i]], range(1, 5));
        $this->ok(['token' => 'v1:s1/p0@2']);
        $this->ok(['token' => 'v1:s1/p1@2']);
        $this->ok(['token' => 'v1:s1/p0@5']);
        $result = $client->documents()->writeBatch('notes', $docs, chunkRows: 2);
        $this->assertSame('v1:s1/p0@5,s1/p1@2', $result->token);
        $this->assertSame([3, 5], [$result->chunks, $result->rows]);
        $this->assertSame([2, 2, 1], array_map(static fn ($r) => count($r['body']['ops']), $this->seen));
        $keys = array_map(static fn ($r) => $r['headers']['idempotency-key'], $this->seen);
        $this->assertCount(3, array_unique($keys), 'every chunk has its own key');
        $this->assertSame($keys, array_map(static fn ($r) => $r['body']['idempotencyKey'], $this->seen));

        $this->seen = [];
        $this->ok(['token' => 'v1:s1/p0@2']);
        $this->refuse(503, 'unavailable', 'down');
        $this->ok(['token' => 'v1:s1/p0@4']);
        $this->refuse(400, 'invalid_argument', 'bad doc');
        try {
            $client->documents()->writeBatch('notes', $docs, chunkRows: 2);
            $this->fail('the failed chunk did not stop the batch');
        } catch (BatchException $e) {
            $this->assertSame(2, $e->chunksApplied);
            $this->assertSame(4, $e->rowsApplied);
            $this->assertSame('v1:s1/p0@4', $e->token);
            $this->assertSame('2', $e->metadata['chunks_applied']);
            $this->assertSame('batch_partially_applied', $e->reason);
            $this->assertSame('aborted', $e->errorCode);
            $this->assertSame('invalid_argument', $e->cause->errorCode);
        }
        $this->assertSame($this->seen[1]['headers']['idempotency-key'], $this->seen[2]['headers']['idempotency-key'], 'a retried chunk reuses its key');

        $this->seen = [];
        for ($i = 0; $i < 5; $i++) {
            $this->ok();
        }
        $client->documents()->writeBatch('notes', $docs, chunkBytes: 60);
        $this->assertGreaterThan(1, count($this->seen), 'the byte budget closes a chunk');
        $this->answers = [];
    }

    #[Test]
    public function php_streams_subscribe_resumes(): void
    {
        $log = array_map(static fn (int $i) => ['offset' => $i, 'value' => "r{$i}"], range(0, 4));
        $fetched = [];
        $sleeps = [];
        $sub = new Subscription(function (int $offset) use (&$fetched, $log): array {
            $fetched[] = $offset;
            if (count($fetched) === 2 || count($fetched) === 3) {
                return [];
            }
            return array_slice(array_values(array_filter($log, static fn ($r) => $r['offset'] >= $offset)), 0, 2);
        }, fromOffset: 1, sleeper: function (float $s) use (&$sleeps): void {
            $sleeps[] = $s;
        });
        $got = [];
        foreach ($sub as $record) {
            $got[] = $record['value'];
            if (count($got) === 4) {
                $sub->cancel();
            }
        }
        $this->assertSame(['r1', 'r2', 'r3', 'r4'], $got);
        $this->assertSame(5, $sub->offset(), 'the next offset is the last offset + 1');
        $this->assertSame([1, 3, 3, 3], $fetched);
        $this->assertSame([0.1, 0.2], $sleeps, 'an empty fetch backs off, doubling');

        $capped = [];
        $empty = null;
        $empty = new Subscription(static fn (int $o) => [], sleeper: function (float $s) use (&$capped, &$empty): void {
            $capped[] = $s;
            if (count($capped) === 6) {
                $empty->cancel();
            }
        });
        foreach ($empty as $_) {
            $this->fail('nothing to yield');
        }
        $this->assertSame([0.1, 0.2, 0.4, 0.8, 1.0, 1.0], $capped, 'the backoff stops at 1 s');
    }

    #[Test]
    public function php_builders_match_corpus(): void
    {
        $this->assertSame([
            'fields' => [
                ['name' => 'title', 'source_path' => 'title', 'kind' => ['text' => ['analyzer' => 'standard', 'positions' => true]], 'indexed' => true, 'fast' => false],
                ['name' => 'tags', 'source_path' => 'tags', 'kind' => 'keyword', 'indexed' => true, 'fast' => true],
                ['name' => 'n', 'source_path' => 'n', 'kind' => 'i64', 'indexed' => true, 'fast' => true],
            ],
            'vectors' => [['name' => 'embedding', 'dim' => 4, 'distance' => 'cosine']],
            'sparse_vectors' => [],
            'dynamic' => 'ignore',
            'max_fields' => 1000,
        ], Schema::new()->text('title')->keyword('tags')->i64('n')->vector('embedding', 4)->toArray(), 'schema_notes');
        $this->assertSame(['term' => ['field' => 'tags', 'value' => 'policy']], Filter::eq('tags', 'policy')->toArray(), 'filter_term');
        $this->assertSame(['bool' => ['must' => [
            ['term' => ['field' => 'tags', 'value' => 'policy']],
            ['bool' => ['should' => [['range' => ['field' => 'n', 'gte' => 2]], ['bool' => ['mustNot' => [['exists' => ['field' => 'archived']]]]]]]],
        ]]], self::canon(Filter::and(Filter::eq('tags', 'policy'), Filter::or(Filter::range('n', gte: 2), Filter::not(Filter::exists('archived'))))), 'filter_bool_nested');
        $this->assertSame(['range' => ['field' => 'n', 'gt' => 1]], Filter::range('n', gt: 1)->toArray(), 'filter_range_open');
        $this->assertSame(['ids' => [['uint' => '1'], 'k-str', ['uuid' => '0190f5c4-6c1e-7b3a-9d2e-4f5a6b7c8d9e']]],
            Filter::ids(1, 'k-str', ['uuid' => '0190f5c4-6c1e-7b3a-9d2e-4f5a6b7c8d9e'])->toArray(), 'filter_ids');
        $this->assertSame('{"matchAll":{}}', Json::encode(Filter::all()), 'an empty message is {}');
        $this->assertSame(['bool' => ['should' => [['matchAll' => []], ['matchNone' => []]], 'minimumShouldMatch' => '1']], self::canon(Filter::anyOf([Filter::all(), Filter::none()], 1)));
        $this->assertSame(['multiMatch' => ['fields' => [['field' => 'title', 'boost' => 2.0], ['field' => 'body']], 'text' => 'q']], Filter::multiMatch(['title^2', 'body'], 'q')->toArray());

        $text = ['text' => ['query' => ['match' => ['field' => 'body', 'text' => 'refund']]]];
        $this->assertSame([
            'collection' => 'notes', 'retrievers' => [$text],
            'filter' => ['term' => ['field' => 'tags', 'value' => 'policy']],
            'highlight' => ['fields' => [['field' => 'body']]],
            'select' => ['source' => ['include' => ['title']]],
            'limit' => 2, 'sort' => [['score' => []], ['pk' => []]],
        ], self::canon(Search::on('notes')->text('refund', ['body'])->filter(Filter::eq('tags', 'policy'))->highlight('body')
            ->select(include: ['title'])->limit(2)->sort('score', 'pk')), 'search_text_filter_highlight_select');
        $this->assertSame(['collection' => 'notes', 'retrievers' => [['vector' => ['field' => 'embedding', 'query' => [1.0, 0.0, 0.0, 0.0], 'k' => 3, 'params' => ['nprobes' => 8, 'refineFactor' => 2]]]], 'limit' => 3],
            Search::on('notes')->vector('embedding', [1, 0, 0, 0], k: 3, nprobes: 8, refineFactor: 2)->limit(3)->toArray(), 'search_vector_ann_params');
        $hybrid = static fn (array $fusion) => self::canon(Search::on('notes')->text('refund', ['body'])->vector('embedding', [1, 0, 0, 0], 5)->fuse($fusion)->limit(3));
        $vec = ['vector' => ['field' => 'embedding', 'query' => [1.0, 0.0, 0.0, 0.0], 'k' => 5]];
        $this->assertSame(['collection' => 'notes', 'retrievers' => [$text, $vec], 'fusion' => ['rrf' => ['k' => 60]], 'limit' => 3], $hybrid(Fusion::rrf(60)), 'search_hybrid_rrf');
        $this->assertSame('{"dbsf":{}}', Json::encode(Fusion::dbsf()), 'search_hybrid_dbsf');
        $this->assertSame('{"rrf":{}}', Json::encode(Fusion::rrf()));
        $this->assertSame(['weightedSum' => ['weights' => [0.7, 0.3]]], $hybrid(Fusion::weighted(0.7, 0.3))['fusion'], 'search_hybrid_weighted');
        $this->assertSame([1.25, 2], Search::on('notes')->text('refund')->sort('score', 'pk')->after([1.25, 2])->toArray()['searchAfter'], 'search_after_page');
        $this->assertSame(['graph' => 'notes_graph', 'fromField' => 'id', 'hops' => 1], Search::on('notes')->expand('notes_graph', fromField: 'id', hops: 1)->toArray()['expandSpec'], 'search_expand');
        $this->assertSame(['atLeast' => 'v1:s1/p0@2'], Search::on('n')->consistency('v1:s1/p0@2')->toArray()['consistency']);
        $this->assertSame(['freshness' => 'FRESHNESS_STRONG'], Search::on('n')->consistency('strong')->toArray()['consistency']);
        $this->assertSame('none', Search::on('n')->select(noSource: true)->toArray()['select']['source']);
        try {
            Search::on('n')->consistency('not-a-token');
            $this->fail('a malformed token was accepted');
        } catch (InvalidArgumentException) {
        }
        try {
            Search::on('n')->text('x')->vector('v', [1])->fuse(Fusion::weighted(0.7))->build();
            $this->fail('one weight for two retrievers was accepted');
        } catch (InvalidArgumentException $e) {
            $this->assertSame('invalid_argument', $e->errorCode);
        }
        $wire = Search::wire(Search::on('n')->text('x')->vector('v', [1], 3)->limit(25));
        $this->assertSame([null, 3], [$wire['retrievers'][0]['text']['k'] ?? null, $wire['retrievers'][1]['vector']['k']], 'k goes out only when given');

        $cases = BuilderCorpus::cases();
        if ($cases === []) {
            $this->markTestSkipped('no builder corpus in ' . BuilderCorpus::dir());
        }
        foreach ($cases as $c) {
            $got = BuilderCorpus::build($c);
            $this->assertEquals(BuilderCorpus::normalize($c['expect']), BuilderCorpus::normalize(self::canon($got)), $c['name']);
        }
    }

    #[Test]
    public function php_collection_handle_delegates(): void
    {
        $client = $this->client(namespace: 'app');
        $notes = $client->collection('notes', 'other');
        $this->ok();
        $this->ok();
        $this->ok(['count' => '3']);
        $this->ok();
        $this->ok();
        $this->ok();
        $this->ok(['dropped' => true]);
        $notes->upsert([['id' => 'a', 'source' => ['x' => 1]]]);
        $notes->get(['a']);
        $this->assertSame('3', $notes->count()['count']);
        $notes->search(Search::on('ignored')->text('x', ['body']));
        $notes->vectorSearch([1, 0], 'embedding');
        $notes->info();
        $this->assertTrue($notes->drop()['dropped']);
        $this->assertSame(['WriteDocuments', 'GetDocuments', 'CountDocuments', 'Search', 'Search', 'GetCollection', 'DropCollection'],
            array_map(fn (int $i) => $this->method($i), array_keys($this->seen)));
        foreach ($this->seen as $r) {
            $this->assertSame('other', $r['body']['namespace']);
            $this->assertSame('notes', $r['body']['collection']);
        }
    }

    #[Test]
    public function php_search_query_all_requires_tiebreak(): void
    {
        $client = $this->client(namespace: 'app');
        try {
            iterator_to_array($client->search()->queryAll(Search::on('notes')->text('x')->sort('score')));
            $this->fail('no tiebreak was refused');
        } catch (InvalidArgumentException $e) {
            $this->assertSame('invalid_argument', $e->errorCode);
        }
        $this->assertSame([], $this->seen);

        $hit = static fn (int $pk, array $sv) => ['pk' => ['uint' => (string) $pk], 'sortValues' => $sv];
        $this->ok(['hits' => [$hit(1, [0.9, 1]), $hit(2, [0.8, 2])]]);
        $this->ok(['hits' => [$hit(3, [0.7, 3])]]);
        $hits = iterator_to_array($client->search()->queryAll(Search::on('notes')->text('x')->sort('score', 'pk')->limit(2)), false);
        $this->assertSame(['1', '2', '3'], array_map(static fn ($h) => $h['pk']['uint'], $hits));
        $this->assertSame([0.8, 2], $this->seen[1]['body']['searchAfter'], "the last hit's sortValues, as answered");
    }

    #[Test]
    public function php_sql_and_streams_send_the_recorded_bodies(): void
    {
        $client = $this->client(namespace: 'app');
        for ($i = 0; $i < 7; $i++) {
            $this->ok(['columns' => [['name' => 'tags'], ['name' => 'c']], 'rows' => [['policy', 2]]]);
        }
        $answer = $client->sql()->query('SELECT tags, count(*) AS c FROM notes WHERE n > $1 GROUP BY tags ORDER BY tags', [1]);
        $this->assertEquals(['namespace' => 'app', 'sql' => 'SELECT tags, count(*) AS c FROM notes WHERE n > $1 GROUP BY tags ORDER BY tags', 'params' => [1]], $this->seen[0]['body']);
        $this->assertSame([['tags' => 'policy', 'c' => 2]], \Loams\Modules\Sql::records($answer));
        $client->sql()->query('SELECT title, n FROM notes ORDER BY n', maxRows: 2);
        $this->assertEquals(['namespace' => 'app', 'sql' => 'SELECT title, n FROM notes ORDER BY n', 'maxRows' => 2], $this->seen[1]['body']);
        $client->streams()->create('note-events');
        $this->assertEquals(['namespace' => 'app', 'name' => 'note-events', 'partitions' => 1], $this->seen[2]['body']);
        $client->streams()->produce('note-events', [['key' => '1', 'value' => ['note' => 1, 'op' => 'created']], ['key' => '5', 'value' => ['note' => 5, 'op' => 'deleted']]], producerId: 'app-producer-1', producerSeq: 1);
        $this->assertEquals(['namespace' => 'app', 'stream' => 'note-events', 'records' => [
            ['key' => 'MQ==', 'value' => 'eyJub3RlIjoxLCJvcCI6ImNyZWF0ZWQifQ=='],
            ['key' => 'NQ==', 'value' => 'eyJub3RlIjo1LCJvcCI6ImRlbGV0ZWQifQ=='],
        ], 'producerId' => 'app-producer-1', 'producerSeq' => '1'], $this->seen[3]['body']);
        $client->streams()->fetch('note-events', maxRecords: 10);
        $this->assertEquals(['namespace' => 'app', 'stream' => 'note-events', 'maxRecords' => 10], $this->seen[4]['body'], 'a zero partition and offset are omitted');
        $client->streams()->fetch('note-events', offset: 2, maxRecords: 10);
        $this->assertEquals(['namespace' => 'app', 'stream' => 'note-events', 'offset' => '2', 'maxRecords' => 10], $this->seen[5]['body']);
        $client->streams()->get('note-events');
        $this->assertEquals(['namespace' => 'app', 'stream' => 'note-events'], $this->seen[6]['body']);
    }

    #[Test]
    public function php_streams_module_subscribe(): void
    {
        $client = $this->client(namespace: 'app');
        $this->ok(['records' => [['offset' => '1', 'value' => base64_encode('a')], ['offset' => '2', 'value' => base64_encode('b')]]]);
        $this->ok(['records' => [['offset' => '3', 'value' => base64_encode('c')]]]);
        $sub = $client->streams()->subscribe('events', fromOffset: 1);
        $got = [];
        foreach ($sub as $record) {
            $got[] = $record['value'];
            if (count($got) === 3) {
                $sub->cancel();
            }
        }
        $this->assertSame(['a', 'b', 'c'], $got);
        $this->assertSame(4, $sub->offset());
        $this->assertSame('1', $this->seen[0]['body']['offset']);
        $this->assertSame('3', $this->seen[1]['body']['offset'], 'the next fetch starts after the last record');
    }

    #[Test]
    public function php_facade_matches_index(): void
    {
        $client = $this->client();
        foreach (Calls::CALLS as $name => $call) {
            [$module, $fn] = explode('.', $name);
            $camel = lcfirst(str_replace('_', '', ucwords($fn, '_')));
            $this->assertTrue(method_exists($client->{$module}(), $camel), "{$name}");
            $this->assertMatchesRegularExpression('/^loams\.(collection|graph|sql|stream)\.v1\.\w+Service\/\w+$/', $call['rpc']);
        }
        $path = __DIR__ . '/../../fixtures/facade-index.json';
        if (!is_file($path)) {
            return;
        }
        $index = json_decode((string) file_get_contents($path), true);
        $modules = array_unique(array_map(static fn ($n) => explode('.', $n)[0], array_keys(Calls::CALLS)));
        foreach ($index['calls'] as $c) {
            if (!in_array($c['module'], $modules, true) || ($c['stream'] ?? false)) {
                continue;
            }
            $snake = strtolower((string) preg_replace('/(?<!^)[A-Z]/', '_$0', $c['call']));
            $key = "{$c['module']}.{$snake}";
            $this->assertArrayHasKey($key, Calls::CALLS, "{$key} is in the facade");
            $this->assertSame($c['rpc'], Calls::CALLS[$key]['rpc'], "{$key} rpc");
            if (isset($c['retry'])) {
                $this->assertSame($c['retry'], Calls::CALLS[$key]['retry'], "{$key} retry class");
            }
        }
    }

    #[Test]
    public function php_example_notes_compiles(): void
    {
        $files = glob(__DIR__ . '/../examples/*/*.php') ?: [];
        $this->assertNotEmpty($files);
        foreach ($files as $file) {
            exec(escapeshellarg(PHP_BINARY) . ' -l ' . escapeshellarg($file), $out, $code);
            $this->assertSame(0, $code, implode("\n", $out));
        }
    }
}

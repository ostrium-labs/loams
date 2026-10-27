<?php

declare(strict_types=1);

namespace Loams\Tests;

use Loams\AlreadyExistsException;
use Loams\Builders\Filter;
use Loams\Builders\Fusion;
use Loams\Builders\Schema;
use Loams\Builders\Search;
use Loams\Client;
use Loams\ConsistencyToken;
use Loams\FeatureNotInVariantException;
use Loams\Idempotency;
use Loams\InvalidArgumentException;
use Loams\LoamsException;
use Loams\Modules\Graph;
use Loams\NotFoundException;
use Loams\Values;
use PHPUnit\Framework\Attributes\Test;
use PHPUnit\Framework\TestCase;

/**
 * The SDKP app scenario through the public facade, against a running server:
 * `LOAMS_TEST_ENDPOINT=http://… vendor/bin/phpunit`. Skipped without one.
 */
final class AppLiveTest extends TestCase
{
    private const NOTES = [
        ['id' => 1, 'source' => ['title' => 'Refunds', 'body' => 'refund policy for orders', 'tags' => ['policy'], 'n' => 1], 'vectors' => ['embedding' => [1.0, 0.0, 0.0, 0.0]]],
        ['id' => 2, 'source' => ['title' => 'Shipping', 'body' => 'shipping times and carriers', 'tags' => ['ops'], 'n' => 2], 'vectors' => ['embedding' => [0.0, 1.0, 0.0, 0.0]]],
        ['id' => 3, 'source' => ['title' => 'Refund window', 'body' => 'the refund window is thirty days', 'tags' => ['policy'], 'n' => 3], 'vectors' => ['embedding' => [0.9, 0.1, 0.0, 0.0]]],
        ['id' => 4, 'source' => ['title' => 'Returns', 'body' => 'returns need a receipt', 'tags' => ['policy', 'ops'], 'n' => 4], 'vectors' => ['embedding' => [0.5, 0.5, 0.0, 0.0]]],
        ['id' => 5, 'source' => ['title' => 'Holidays', 'body' => 'holiday hours', 'tags' => ['ops'], 'n' => 5], 'vectors' => ['embedding' => [0.0, 0.0, 1.0, 0.0]]],
    ];

    private Client $client;
    private string $ns;

    protected function setUp(): void
    {
        $endpoint = (string) getenv('LOAMS_TEST_ENDPOINT');
        if ($endpoint === '') {
            $this->markTestSkipped('LOAMS_TEST_ENDPOINT is not set');
        }
        $this->ns = 'parity-php-' . (int) (microtime(true) * 1000) . '-' . random_int(0, 999);
        $key = getenv('LOAMS_TEST_API_KEY');
        $this->client = new Client($endpoint, apiKey: $key === false ? null : $key, namespace: $this->ns, sessionConsistency: true);
    }

    private static function schema(): Schema
    {
        return Schema::new()->keyword('title')->text('body')->keyword('tags')->i64('n')->vector('embedding', 4)->dynamic('ignore');
    }

    #[Test]
    public function php_live_app_collections(): void
    {
        $this->assertNotEmpty($this->client->instances->getInstance());
        $this->client->namespaces()->create($this->ns);
        try {
            $this->client->namespaces()->create($this->ns);
            $this->fail('a second create was not refused');
        } catch (AlreadyExistsException $e) {
            $this->assertSame('already_exists', $e->errorCode);
        }
        $this->assertContains($this->ns, array_column(iterator_to_array($this->client->namespaces()->listAll(), false), 'namespace'));

        $this->assertSame('notes', $this->client->collections()->create('notes', self::schema())['name']);
        $this->assertSame('notes', $this->client->collections()->get('notes')['name']);
        $this->client->collections()->create('scratch', Schema::new()->keyword('k'));
        $names = array_column(iterator_to_array($this->client->collections()->listAll(pageSize: 1), false), 'name');
        sort($names);
        $this->assertSame(['notes', 'scratch'], $names);

        $key = Idempotency::mintKey();
        $first = $this->client->documents()->upsert('notes', self::NOTES, options: ['idempotencyKey' => $key]);
        $again = $this->client->documents()->upsert('notes', self::NOTES, options: ['idempotencyKey' => $key]);
        $this->assertSame($first['token'], $again['token'], 'the same key answers the same write (R3)');

        $got = $this->client->documents()->get('notes', [1, 2], consistency: $first['token']);
        $this->assertSame(['Refunds', 'Shipping'], array_map(static fn ($d) => $d['source']['title'], $got['documents']));

        $this->client->documents()->patch('notes', 4, set: ['title' => 'Returns and exchanges']);
        $this->client->documents()->delete('notes', [5]);
        // The native filter JSON until P0a types `where`.
        $this->assertSame('3', $this->client->documents()->count('notes', filter: ['term' => ['field' => 'tags', 'value' => 'policy']])['count']);

        $ids = array_map(static fn ($d) => Values::fromDocumentId($d['id']), iterator_to_array($this->client->documents()->scrollAll('notes', limit: 2), false));
        sort($ids);
        $this->assertSame([1, 2, 3, 4], $ids);
        $this->assertSame('Returns and exchanges', $this->client->documents()->get('notes', [4])['documents'][0]['source']['title']);

        try {
            $this->client->collections()->get('missing');
            $this->fail('a missing collection was found');
        } catch (NotFoundException $e) {
            $this->assertSame('not_found', $e->errorCode);
        }
        $this->assertTrue($this->client->collections()->drop('scratch')['dropped']);

        $handle = $this->client->collection('notes');
        $this->assertSame('notes', $handle->info()['name']);
        $docs = array_map(static fn (int $i) => ['id' => $i, 'source' => ['title' => "b{$i}", 'n' => $i]], range(10, 14));
        $result = $handle->writeBatch($docs, chunkRows: 2);
        $this->assertSame([3, 5], [$result->chunks, $result->rows]);
        $this->assertSame('9', $handle->count(consistency: $result->token)['count']);
    }

    #[Test]
    public function php_live_app_search(): void
    {
        $this->client->namespaces()->create($this->ns);
        $this->client->collections()->create('notes', self::schema());
        $this->client->documents()->upsert('notes', self::NOTES);

        $page = $this->client->search()->query(
            Search::on('notes')->text('refund', ['body'])->filter(Filter::eq('tags', 'policy'))
                ->highlight('body')->select(include: ['title'])->limit(2)->sort('score', 'pk')
        );
        $this->assertCount(2, $page['hits']);
        $this->assertSame(['title'], array_keys($page['hits'][0]['source']));
        $this->assertNotEmpty($page['hits'][0]['highlight']);

        $all = iterator_to_array($this->client->search()->queryAll(Search::on('notes')->text('refund', ['body'])->sort('score', 'pk')->limit(1)), false);
        $ids = array_map(static fn ($h) => Values::fromDocumentId($h['pk']), $all);
        sort($ids);
        $this->assertSame([1, 3], $ids);

        $near = $this->client->vector()->search('notes', [1, 0, 0, 0], 'embedding', limit: 2);
        $this->assertSame(1, Values::fromDocumentId($near['hits'][0]['pk']));

        foreach ([Fusion::rrf(60), Fusion::dbsf(), Fusion::weighted(0.7, 0.3)] as $fusion) {
            $res = $this->client->search()->hybrid('notes', 'refund', [1, 0, 0, 0], 'embedding', ['body'], $fusion, filter: Filter::eq('tags', 'policy'), limit: 3);
            $this->assertNotEmpty($res['hits'] ?? [], (string) array_key_first($fusion));
        }
        try {
            $this->client->search()->hybrid('notes', 'refund', [1, 0, 0, 0], 'embedding', ['body'], Fusion::weighted(0.7));
            $this->fail('the wrong number of weights was accepted');
        } catch (InvalidArgumentException $e) {
            $this->assertSame('invalid_argument', $e->errorCode);
        }

        $hits = $this->client->collection('notes')->search(configure: static fn (Search $s) => $s->text('refund', ['body'])->limit(5))['hits'];
        $this->assertCount(2, $hits);
    }

    #[Test]
    public function php_live_app_graph(): void
    {
        $this->client->namespaces()->create($this->ns);
        try {
            $this->client->graph()->create('notes_graph');
        } catch (LoamsException $e) {
            if ($e->reason === 'feature_not_in_variant' || $e instanceof FeatureNotInVariantException) {
                $this->markTestSkipped('this build serves no graph');
            }
            throw $e;
        }
        $this->client->graph()->query('notes_graph', 'INSERT (:Note {id: $id, title: $title})-[:TAGGED]->(:Tag {name: $tag})',
            ['id' => 1, 'title' => 'Refunds', 'tag' => 'policy']);
        $read = $this->client->graph()->query('notes_graph', 'MATCH (n:Note)-[:TAGGED]->(t:Tag) RETURN n.title AS title, t.name AS tag', readOnly: true);
        $this->assertSame([['title' => 'Refunds', 'tag' => 'policy']], Graph::records($read));
        $this->assertContains('notes_graph', array_column(iterator_to_array($this->client->graph()->listAll(), false), 'name'));
        $this->assertNotEmpty($this->client->graph()->delete('notes_graph'));
    }
}

<?php

// A notes app on Loams: a namespace, a collection with a schema, documents,
// a hybrid search with a filter, and a graph query.
//
//   LOAMS_ENDPOINT=http://127.0.0.1:8080 LOAMS_NAMESPACE=app php examples/notes/notes.php

declare(strict_types=1);

require __DIR__ . '/../bootstrap.php';

use Loams\AlreadyExistsException;
use Loams\Builders\Filter;
use Loams\Builders\Fusion;
use Loams\Builders\Schema;
use Loams\Builders\Search;
use Loams\Client;
use Loams\FeatureNotInVariantException;
use Loams\LoamsException;
use Loams\Modules\Graph;
use Loams\Values;

$client = Client::fromEnv(sessionConsistency: true);

try {
    $client->namespaces()->create((string) $client->namespace);
} catch (AlreadyExistsException) {
    // Created by an earlier run.
}

$schema = Schema::new()->keyword('title')->text('body')->keyword('tags')->i64('n')->vector('embedding', 4);
try {
    $client->collections()->create('notes', $schema);
} catch (AlreadyExistsException) {
    // Created by an earlier run.
}

$notes = $client->collection('notes');
$notes->upsert([
    ['id' => 1, 'source' => ['title' => 'Refunds', 'body' => 'refund policy for orders', 'tags' => ['policy'], 'n' => 1], 'vectors' => ['embedding' => [1, 0, 0, 0]]],
    ['id' => 2, 'source' => ['title' => 'Shipping', 'body' => 'shipping times', 'tags' => ['ops'], 'n' => 2], 'vectors' => ['embedding' => [0, 1, 0, 0]]],
    ['id' => 3, 'source' => ['title' => 'Refund window', 'body' => 'the refund window is thirty days', 'tags' => ['policy'], 'n' => 3], 'vectors' => ['embedding' => [0.9, 0.1, 0, 0]]],
]);

// With sessionConsistency, this read sees the write above.
$search = Search::on('notes')
    // k is explicit so the example also runs on a 0.1.0 engine, which has no default k.
    ->text('refund', ['body'], k: 10)
    ->vector('embedding', [1, 0, 0, 0], k: 10)
    ->fuse(Fusion::rrf(60))
    ->filter(Filter::eq('tags', 'policy'))
    ->highlight('body')
    ->select(include: ['title'])
    ->limit(5);
foreach ($notes->search($search)['hits'] ?? [] as $hit) {
    printf("%s: %s (%.3f)\n", Values::fromDocumentId($hit['pk']), $hit['source']['title'], $hit['score'] ?? 0);
}

printf("%s notes\n", $notes->count()['count'] ?? 0);

// Graph is experimental and not in every build.
try {
    $client->graph()->create('notes_graph');
    $client->graph()->query('notes_graph', 'INSERT (:Note {id: $id})-[:TAGGED]->(:Tag {name: $tag})', ['id' => 1, 'tag' => 'policy']);
    $rows = $client->graph()->query('notes_graph', 'MATCH (n:Note)-[:TAGGED]->(t:Tag) RETURN n.id AS id, t.name AS tag', readOnly: true);
    var_export(Graph::records($rows));
    echo "\n";
    $client->graph()->delete('notes_graph');
} catch (FeatureNotInVariantException) {
    echo "this server has no graph\n";
} catch (LoamsException $e) {
    if ($e->reason !== 'feature_not_in_variant') {
        throw $e;
    }
    echo "this server has no graph\n";
}

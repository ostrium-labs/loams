<?php

declare(strict_types=1);

namespace Loams\Modules;

use Loams\Builders\Json;
use Loams\Client;
use Loams\Idempotency;
use Loams\InvalidArgumentException;

/**
 * The RPCs behind every app call (SDKP §2), `module.call` to its RPC and
 * retry class (D610: `safe`, `keyed` or `never`). Checked against
 * `sdks/fixtures/facade-index.json` by `php_facade_matches_index`.
 */
final class Calls
{
    /** @var array<string, array{rpc: string, retry: string, namespace: bool, read: bool}> */
    public const CALLS = [
        'namespaces.create' => ['rpc' => 'loams.collection.v1.NamespaceService/CreateNamespace', 'retry' => 'safe', 'namespace' => false, 'read' => false],
        'namespaces.list' => ['rpc' => 'loams.collection.v1.NamespaceService/ListNamespaces', 'retry' => 'safe', 'namespace' => false, 'read' => false],
        'collections.create' => ['rpc' => 'loams.collection.v1.CollectionService/CreateCollection', 'retry' => 'safe', 'namespace' => true, 'read' => false],
        'collections.list' => ['rpc' => 'loams.collection.v1.CollectionService/ListCollections', 'retry' => 'safe', 'namespace' => true, 'read' => false],
        'collections.get' => ['rpc' => 'loams.collection.v1.CollectionService/GetCollection', 'retry' => 'safe', 'namespace' => true, 'read' => false],
        'collections.drop' => ['rpc' => 'loams.collection.v1.CollectionService/DropCollection', 'retry' => 'safe', 'namespace' => true, 'read' => false],
        'collections.add_fields' => ['rpc' => 'loams.collection.v1.CollectionService/AddFields', 'retry' => 'safe', 'namespace' => true, 'read' => false],
        'collections.list_versions' => ['rpc' => 'loams.collection.v1.CollectionService/ListVersions', 'retry' => 'safe', 'namespace' => true, 'read' => false],
        'collections.update_aliases' => ['rpc' => 'loams.collection.v1.CollectionService/UpdateAliases', 'retry' => 'safe', 'namespace' => true, 'read' => false],
        'collections.scan' => ['rpc' => 'loams.collection.v1.CollectionService/Scan', 'retry' => 'never', 'namespace' => true, 'read' => false],
        'collections.set_hot' => ['rpc' => 'loams.collection.v1.CollectionService/SetHot', 'retry' => 'safe', 'namespace' => true, 'read' => false],
        'collections.warm' => ['rpc' => 'loams.collection.v1.CollectionService/WarmCollection', 'retry' => 'safe', 'namespace' => true, 'read' => false],
        'documents.write' => ['rpc' => 'loams.collection.v1.DocumentService/WriteDocuments', 'retry' => 'keyed', 'namespace' => true, 'read' => false],
        'documents.get' => ['rpc' => 'loams.collection.v1.DocumentService/GetDocuments', 'retry' => 'safe', 'namespace' => true, 'read' => true],
        'documents.scroll' => ['rpc' => 'loams.collection.v1.DocumentService/ScrollDocuments', 'retry' => 'safe', 'namespace' => true, 'read' => true],
        'documents.count' => ['rpc' => 'loams.collection.v1.DocumentService/CountDocuments', 'retry' => 'safe', 'namespace' => true, 'read' => true],
        'documents.delete_by_filter' => ['rpc' => 'loams.collection.v1.DocumentService/DeleteByFilter', 'retry' => 'never', 'namespace' => true, 'read' => true],
        'documents.patch_by_filter' => ['rpc' => 'loams.collection.v1.DocumentService/PatchByFilter', 'retry' => 'never', 'namespace' => true, 'read' => true],
        'search.query' => ['rpc' => 'loams.collection.v1.QueryService/Search', 'retry' => 'safe', 'namespace' => true, 'read' => true],
        'vector.search' => ['rpc' => 'loams.collection.v1.QueryService/Search', 'retry' => 'safe', 'namespace' => true, 'read' => true],
        'graph.create' => ['rpc' => 'loams.graph.v1.GraphAdminService/CreateGraph', 'retry' => 'keyed', 'namespace' => true, 'read' => false],
        'graph.get' => ['rpc' => 'loams.graph.v1.GraphAdminService/GetGraph', 'retry' => 'safe', 'namespace' => true, 'read' => false],
        'graph.list' => ['rpc' => 'loams.graph.v1.GraphAdminService/ListGraphs', 'retry' => 'safe', 'namespace' => true, 'read' => false],
        'graph.update' => ['rpc' => 'loams.graph.v1.GraphAdminService/UpdateGraph', 'retry' => 'keyed', 'namespace' => false, 'read' => false],
        'graph.delete' => ['rpc' => 'loams.graph.v1.GraphAdminService/DeleteGraph', 'retry' => 'keyed', 'namespace' => true, 'read' => false],
        'graph.schema' => ['rpc' => 'loams.graph.v1.GraphAdminService/GetSchema', 'retry' => 'safe', 'namespace' => true, 'read' => false],
        'graph.query' => ['rpc' => 'loams.graph.v1.GraphService/Execute', 'retry' => 'keyed', 'namespace' => true, 'read' => true],
        'graph.query_batch' => ['rpc' => 'loams.graph.v1.GraphService/ExecuteBatch', 'retry' => 'keyed', 'namespace' => true, 'read' => false],
        'graph.explain' => ['rpc' => 'loams.graph.v1.GraphService/Explain', 'retry' => 'safe', 'namespace' => true, 'read' => true],
        'sql.query' => ['rpc' => 'loams.sql.v1.SqlService/Query', 'retry' => 'safe', 'namespace' => true, 'read' => true],
        'sql.query_arrow' => ['rpc' => 'loams.sql.v1.SqlService/QueryArrow', 'retry' => 'safe', 'namespace' => true, 'read' => true],
        'streams.create' => ['rpc' => 'loams.stream.v1.StreamService/CreateStream', 'retry' => 'safe', 'namespace' => true, 'read' => false],
        'streams.get' => ['rpc' => 'loams.stream.v1.StreamService/DescribeStream', 'retry' => 'safe', 'namespace' => true, 'read' => false],
        'streams.list' => ['rpc' => 'loams.stream.v1.StreamService/ListStreams', 'retry' => 'safe', 'namespace' => true, 'read' => false],
        'streams.produce' => ['rpc' => 'loams.stream.v1.StreamService/Produce', 'retry' => 'never', 'namespace' => true, 'read' => false],
        'streams.fetch' => ['rpc' => 'loams.stream.v1.StreamService/Fetch', 'retry' => 'safe', 'namespace' => true, 'read' => false],
        'streams.produce_events' => ['rpc' => 'loams.stream.v1.StreamService/ProduceCloudEvents', 'retry' => 'never', 'namespace' => true, 'read' => false],
        'streams.fetch_events' => ['rpc' => 'loams.stream.v1.StreamService/FetchCloudEvents', 'retry' => 'safe', 'namespace' => true, 'read' => false],
    ];
}

/**
 * What every app module shares: proto3 JSON in, proto3 JSON out (arrays with
 * `lowerCamelCase` keys), through the client's transport, so retries, keys,
 * token refresh, timeouts and errors are R1–R12 as for every other call.
 */
abstract class Base
{
    /** The packages whose reads take the session's token (SDKP-17). */
    public const SESSION_SCOPES = ['loams.collection.v1', 'loams.sql.v1'];

    public function __construct(protected readonly Client $client)
    {
    }

    /**
     * Sends `$request` to the call `$name` (a key of `Calls::CALLS`).
     *
     * R11: a request that names no namespace gets the client's default, and
     * with neither it is refused here, before any network call. R4: with
     * `sessionConsistency`, a read that leaves `consistency` unset reads at
     * least the session's token, and every token an answer carries is merged
     * into the session.
     *
     * @param array<string, mixed> $request
     * @param array{timeout?: ?float, idempotencyKey?: ?string, retry?: string, headers?: array<string, string>} $options
     * @return array<string, mixed>
     */
    protected function rpc(string $name, array $request, array $options = []): array
    {
        $call = Calls::CALLS[$name];
        // The session is kept per proto package (`loams.collection.v1`, `loams.graph.v1`).
        // SQL reads collections, so it reads (and adds to) the collection token.
        $service = (string) strstr($call['rpc'], '/', true);
        $scope = substr($service, 0, (int) strrpos($service, '.'));
        if ($scope === 'loams.sql.v1') {
            $scope = 'loams.collection.v1';
        }
        if ($call['namespace']) {
            $request = $this->withNamespace($request, $call['rpc']);
        }
        $session = $this->client->session();
        // SDKP-17: only collection (and SQL) reads take the session's token.
        if ($call['read'] && $session !== null && in_array($scope, self::SESSION_SCOPES, true)) {
            $request = $session->apply($scope, $request);
        }

        $retry = $options['retry'] ?? $call['retry'];
        $key = null;
        if ($retry === 'keyed') {
            $key = $options['idempotencyKey'] ?? $request['idempotencyKey'] ?? Idempotency::mintKey();
            $request['idempotencyKey'] = $key;
        }
        $headers = ['Content-Type' => 'application/json', 'Connect-Protocol-Version' => '1'] + ($options['headers'] ?? []);
        $res = $this->client->transport->callWithRetry(
            '/' . $call['rpc'],
            $headers,
            Json::encode($request === [] ? Json::empty() : $request),
            $retry !== 'safe',
            $key,
            array_key_exists('timeout', $options) ? $options['timeout'] : false
        );
        $answer = json_decode($res['body'], true) ?? [];
        if ($session !== null) {
            $session->observeAnswer($scope, $answer);
        }
        return $answer;
    }

    /**
     * @param array<string, mixed> $request
     * @return array<string, mixed>
     */
    protected function withNamespace(array $request, string $rpc): array
    {
        if (($request['namespace'] ?? '') !== '') {
            return $request;
        }
        $default = $this->client->namespace;
        if ($default === null || $default === '') {
            throw InvalidArgumentException::client("{$rpc} needs a namespace: pass one or give the client one", 'namespace_required');
        }
        $request['namespace'] = $default;
        return $request;
    }

    /** @return array<string, mixed> */
    protected static function ns(?string $namespace): array
    {
        return $namespace === null ? [] : ['namespace' => $namespace];
    }

    /**
     * Every item of a paged list, following `nextPageToken`.
     *
     * @param array<string, mixed> $request
     * @param array<string, mixed> $options
     */
    protected function paged(string $name, array $request, string $items, array $options): \Generator
    {
        $token = '';
        do {
            $page = $this->rpc($name, $request + ($token === '' ? [] : ['pageToken' => $token]), $options);
            foreach ($page[$items] ?? [] as $item) {
                yield $item;
            }
            $next = (string) ($page['nextPageToken'] ?? '');
            if ($next === $token) {
                break;
            }
            $token = $next;
        } while ($token !== '');
    }
}

/**
 * Any app call by its facade-index name over its request's proto3 JSON:
 * what `Client::call()` goes through.
 */
final class Raw extends Base
{
    /**
     * @param array<string, mixed> $request
     * @param array<string, mixed> $options
     * @return array<string, mixed>
     */
    public function call(string $name, array $request, array $options = []): array
    {
        if (!isset(Calls::CALLS[$name])) {
            throw new \InvalidArgumentException("no app call {$name}");
        }
        return $this->rpc($name, $request, $options);
    }
}

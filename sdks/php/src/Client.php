<?php

declare(strict_types=1);

namespace Loams;

class InstanceService
{
    public function __construct(private Transport $transport)
    {
    }

    /**
     * @param array<string, mixed> $request
     * @return array<string, mixed>
     */
    public function getInstance(array $request = []): array
    {
        $body = json_encode($request, JSON_UNESCAPED_SLASHES);
        $headers = [
            'Content-Type' => 'application/json',
            'Connect-Protocol-Version' => '1',
        ];
        $res = $this->transport->callWithRetry('/loams.instance.v1.InstanceService/GetInstance', $headers, $body, false);
        return json_decode($res['body'], true) ?? [];
    }

    /**
     * @return array<string, mixed>
     */
    public function whoAmI(): array
    {
        $headers = [
            'Content-Type' => 'application/json',
            'Connect-Protocol-Version' => '1',
        ];
        $res = $this->transport->callWithRetry('/loams.instance.v1.InstanceService/WhoAmI', $headers, '{}', false);
        return json_decode($res['body'], true) ?? [];
    }
}

class LiveService
{
    public function __construct(private Transport $transport)
    {
    }

    /**
     * @param array<string, mixed> $request
     * @return array<string, mixed>
     */
    public function query(array $request): array
    {
        $body = json_encode($request, JSON_UNESCAPED_SLASHES);
        $headers = [
            'Content-Type' => 'application/json',
            'Connect-Protocol-Version' => '1',
        ];
        $res = $this->transport->callWithRetry('/loams.live.v1.LiveService/Query', $headers, $body, false);
        return json_decode($res['body'], true) ?? [];
    }

    public function watch(): StreamHandle
    {
        $headers = [
            'Content-Type' => 'application/connect+json',
            'Connect-Protocol-Version' => '1',
        ];
        $res = $this->transport->execute('/loams.live.v1.LiveService/Watch', $headers, Envelopes::pack('{}'));
        if ($res['status'] !== 200) {
            throw ErrorParser::parse($res['status'], $res['body']);
        }
        $frames = Envelopes::split($res['body']);
        return new StreamHandle($frames);
    }
}

class ApprovalService
{
    public function __construct(private Transport $transport)
    {
    }

    /**
     * @param array<string, mixed> $request
     * @return array<string, mixed>
     */
    public function decideApproval(array $request, ?string $idempotencyKey = null): array
    {
        $mintedKey = $idempotencyKey ?? $request['idempotencyKey'] ?? $request['idempotency_key'] ?? Idempotency::mintKey();
        $payload = $request;
        $payload['idempotencyKey'] = $mintedKey;

        $body = json_encode($payload, JSON_UNESCAPED_SLASHES);
        $headers = [
            'Content-Type' => 'application/json',
            'Connect-Protocol-Version' => '1',
        ];
        $res = $this->transport->callWithRetry('/loams.approvals.v1.ApprovalService/DecideApproval', $headers, $body, true, $mintedKey);
        return json_decode($res['body'], true) ?? [];
    }

    public function watchApprovals(?string $cursor = null): StreamHandle
    {
        $headers = [
            'Content-Type' => 'application/connect+json',
            'Connect-Protocol-Version' => '1',
        ];
        $req = $cursor !== null ? ['cursor' => $cursor] : (object)[];
        $body = Envelopes::pack(json_encode($req));
        $res = $this->transport->execute('/loams.approvals.v1.ApprovalService/WatchApprovals', $headers, $body);
        if ($res['status'] !== 200) {
            throw ErrorParser::parse($res['status'], $res['body']);
        }
        $frames = Envelopes::split($res['body']);
        return new StreamHandle($frames, null, $cursor);
    }

    /**
     * @param array<string, mixed> $request
     * @return array<string, mixed>
     */
    public function listApprovals(array $request = []): array
    {
        $body = json_encode($request, JSON_UNESCAPED_SLASHES);
        $headers = [
            'Content-Type' => 'application/json',
            'Connect-Protocol-Version' => '1',
        ];
        $res = $this->transport->callWithRetry('/loams.approvals.v1.ApprovalService/ListApprovals', $headers, $body, false);
        return json_decode($res['body'], true) ?? [];
    }
}

class DeviceService
{
    public function __construct(private Transport $transport)
    {
    }

    /**
     * @param array<string, mixed> $request
     * @return array<string, mixed>
     */
    public function sendTestNotification(array $request = []): array
    {
        $body = json_encode($request, JSON_UNESCAPED_SLASHES);
        $headers = [
            'Content-Type' => 'application/json',
            'Connect-Protocol-Version' => '1',
        ];
        $res = $this->transport->callWithRetry('/loams.devices.v1.DeviceService/SendTestNotification', $headers, $body, false);
        return json_decode($res['body'], true) ?? [];
    }
}

class Client
{
    public readonly Transport $transport;
    public readonly InstanceService $instances;
    public readonly LiveService $live;
    public readonly ApprovalService $approvals;
    public readonly DeviceService $devices;
    /** The default namespace (R11), or null. */
    public readonly ?string $namespace;

    private ?SessionStore $session;
    private Modules\Namespaces $namespacesModule;
    private Modules\Collections $collectionsModule;
    private Modules\Documents $documentsModule;
    private Modules\Search $searchModule;
    private Modules\Vector $vectorModule;
    private Modules\Graph $graphModule;
    private Modules\Sql $sqlModule;
    private Modules\Streams $streamsModule;

    /**
     * @param ?string $apiKey sent as `Authorization: Bearer` (R1)
     * @param ?string $namespace the default namespace (R11)
     * @param ?float $timeout the unary default in seconds, null for none (R12)
     * @param bool $sessionConsistency the session store (R4)
     * @param ?callable $sender replaces cURL (see `Transport`)
     */
    public function __construct(
        string $endpoint,
        ?TokenSourceInterface $tokenSource = null,
        int $maxRetries = 3,
        ?string $apiKey = null,
        ?string $namespace = null,
        ?float $timeout = Transport::DEFAULT_TIMEOUT,
        bool $sessionConsistency = false,
        ?callable $sender = null
    ) {
        if ($tokenSource === null && $apiKey !== null && $apiKey !== '') {
            $tokenSource = new StaticTokenSource($apiKey);
        }
        $this->transport = new Transport($endpoint, $tokenSource, $maxRetries, $timeout, $sender);
        $this->namespace = $namespace;
        $this->session = $sessionConsistency ? new SessionStore() : null;
        $this->instances = new InstanceService($this->transport);
        $this->live = new LiveService($this->transport);
        $this->approvals = new ApprovalService($this->transport);
        $this->devices = new DeviceService($this->transport);
        $this->namespacesModule = new Modules\Namespaces($this);
        $this->collectionsModule = new Modules\Collections($this);
        $this->documentsModule = new Modules\Documents($this);
        $this->searchModule = new Modules\Search($this);
        $this->vectorModule = new Modules\Vector($this);
        $this->graphModule = new Modules\Graph($this);
        $this->sqlModule = new Modules\Sql($this);
        $this->streamsModule = new Modules\Streams($this);
    }

    /**
     * A client from the environment (R1): `LOAMS_ENDPOINT`, `LOAMS_API_KEY`
     * or `LOAMS_TOKEN`, and `LOAMS_NAMESPACE`.
     */
    public static function fromEnv(?float $timeout = Transport::DEFAULT_TIMEOUT, bool $sessionConsistency = false, int $maxRetries = 3): self
    {
        $endpoint = (string) getenv('LOAMS_ENDPOINT');
        if ($endpoint === '') {
            throw InvalidArgumentException::client('LOAMS_ENDPOINT is not set');
        }
        $key = (string) getenv('LOAMS_API_KEY');
        $token = (string) getenv('LOAMS_TOKEN');
        $ns = (string) getenv('LOAMS_NAMESPACE');
        return new self(
            $endpoint,
            maxRetries: $maxRetries,
            apiKey: $key !== '' ? $key : ($token !== '' ? $token : null),
            namespace: $ns !== '' ? $ns : null,
            timeout: $timeout,
            sessionConsistency: $sessionConsistency,
        );
    }

    /** The session store, when `sessionConsistency` is on. */
    public function session(): ?SessionStore
    {
        return $this->session;
    }

    public function namespaces(): Modules\Namespaces
    {
        return $this->namespacesModule;
    }

    public function collections(): Modules\Collections
    {
        return $this->collectionsModule;
    }

    public function documents(): Modules\Documents
    {
        return $this->documentsModule;
    }

    public function search(): Modules\Search
    {
        return $this->searchModule;
    }

    public function vector(): Modules\Vector
    {
        return $this->vectorModule;
    }

    /** Experimental: `loams.graph.v1` is unstable. */
    public function graph(): Modules\Graph
    {
        return $this->graphModule;
    }

    /**
     * Any facade call by its facade-index name (`documents.write`,
     * `instance.get_instance`) over its request's proto3 JSON. `$options`
     * takes `timeout`, `idempotencyKey` and `headers`.
     *
     * @param array<string, mixed> $request
     * @param array<string, mixed> $options
     * @return array<string, mixed>
     */
    public function call(string $name, array $request = [], array $options = []): array
    {
        if ($name === 'instance.get_instance' || $name === 'instance.who_am_i') {
            $path = $name === 'instance.get_instance'
                ? '/loams.instance.v1.InstanceService/GetInstance'
                : '/loams.instance.v1.InstanceService/WhoAmI';
            $headers = ['Content-Type' => 'application/json', 'Connect-Protocol-Version' => '1'] + ($options['headers'] ?? []);
            $res = $this->transport->callWithRetry($path, $headers, $request === [] ? '{}' : (string) json_encode($request), false);
            return json_decode($res['body'], true) ?? [];
        }
        return (new Modules\Raw($this))->call($name, $request, $options);
    }

    public function sql(): Modules\Sql
    {
        return $this->sqlModule;
    }

    public function streams(): Modules\Streams
    {
        return $this->streamsModule;
    }

    /** A handle on one collection (SDKP §2.8). */
    public function collection(string $name, ?string $namespace = null): CollectionHandle
    {
        return new CollectionHandle($this, $name, $namespace);
    }
}

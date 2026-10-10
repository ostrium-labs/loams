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

    public function __construct(
        string $endpoint,
        ?TokenSourceInterface $tokenSource = null,
        int $maxRetries = 3
    ) {
        $this->transport = new Transport($endpoint, $tokenSource, $maxRetries);
        $this->instances = new InstanceService($this->transport);
        $this->live = new LiveService($this->transport);
        $this->approvals = new ApprovalService($this->transport);
        $this->devices = new DeviceService($this->transport);
    }
}

<?php

declare(strict_types=1);

namespace Corpus;

use Temporal\Api\Common\V1\WorkflowExecution as ProtoExecution;
use Temporal\Api\Workflowservice\V1\DescribeWorkflowExecutionRequest;
use Temporal\Client\ClientOptions;
use Temporal\Client\GRPC\ServiceClient;
use Temporal\Client\Update\LifecycleStage;
use Temporal\Client\Update\UpdateOptions;
use Temporal\Client\WorkflowClient;
use Temporal\Client\WorkflowOptions;
use Temporal\Client\WorkflowStubInterface;
use Temporal\Common\IdReusePolicy;
use Temporal\Common\WorkflowIdConflictPolicy;
use Temporal\Exception\Client\WorkflowExecutionAlreadyStartedException;
use Temporal\Exception\Client\WorkflowFailedException;
use Temporal\Exception\Client\WorkflowUpdateException;

/**
 * The PHP corpus's starter (DX1 TSDK-php): drives each scenario against a
 * PHP worker running under RoadRunner (worker.php, .rr.yaml) with the fixed
 * workflow ID corpus-php-<scenario>, as the TypeScript and Ruby drivers do,
 * and records what it observed. The Go exporter writes the histories
 * afterwards (`corpus-go export -prefix corpus-php-`).
 *
 * TEMPORAL_API_KEY, when set, is sent as `authorization: Bearer <key>` on an
 * insecure channel: the gateway's profile serves plaintext on the stack's
 * network.
 */
final class Driver
{
    public function __construct(
        private readonly WorkflowClient $client,
        private readonly string $namespace,
    ) {}

    public static function connect(string $address, string $namespace): self
    {
        $service = ServiceClient::create($address);
        $key = \getenv('TEMPORAL_API_KEY');
        if (\is_string($key) && $key !== '') {
            $service = $service->withAuthKey($key);
        }
        $client = WorkflowClient::create($service, (new ClientOptions())->withNamespace($namespace));
        return new self($client, $namespace);
    }

    private function stub(string $name, string $id, ?WorkflowOptions $options = null): WorkflowStubInterface
    {
        $options ??= WorkflowOptions::new();
        return $this->client->newUntypedWorkflowStub(
            $name,
            $options->withTaskQueue(Scenarios::TASK_QUEUE)->withWorkflowId($id),
        );
    }

    private function start(string $name, string $id, array $args = [], ?WorkflowOptions $options = null): WorkflowStubInterface
    {
        $stub = $this->stub($name, $id, $options);
        $this->client->start($stub, ...$args);
        return $stub;
    }

    /** Until the first workflow task has completed (history >= 4 events, none pending). */
    private function waitStarted(string $id): void
    {
        $req = (new DescribeWorkflowExecutionRequest())
            ->setNamespace($this->namespace)
            ->setExecution((new ProtoExecution())->setWorkflowId($id));
        $deadline = \microtime(true) + 60;
        while (true) {
            $d = $this->client->getServiceClient()->DescribeWorkflowExecution($req);
            $len = (int) $d->getWorkflowExecutionInfo()?->getHistoryLength();
            if ($len >= 4 && $d->getPendingWorkflowTask() === null) {
                return;
            }
            if (\microtime(true) > $deadline) {
                throw new \RuntimeException("{$id}: the first workflow task did not complete within 60 s");
            }
            \usleep(200_000);
        }
    }

    private static function describe(\Throwable $e): string
    {
        $s = $e::class . ': ' . $e->getMessage();
        if ($e->getPrevious() !== null) {
            $s .= ' <- ' . self::describe($e->getPrevious());
        }
        return $s;
    }

    /** @return array{scenario: string, workflow_id: string, result?: string, expected: list<string>} */
    public function drive(string $name, string $id): array
    {
        $o = ['scenario' => $name, 'workflow_id' => $id, 'expected' => []];
        $settle = static function (WorkflowStubInterface $stub) use (&$o): void {
            $o['result'] = \json_encode($stub->getResult(), \JSON_THROW_ON_ERROR | \JSON_UNESCAPED_SLASHES);
        };
        switch ($name) {
            case 'hello':
                $settle($this->start($name, $id, ['corpus']));
                break;
            case 'signal_then_complete':
                $h = $this->start($name, $id);
                $this->waitStarted($id);
                $h->signal('go', 's');
                $settle($h);
                break;
            case 'signal_with_start':
                $h = $this->stub($name, $id);
                $this->client->signalWithStart($h, 'go', ['s']);
                $settle($h);
                break;
            case 'query_while_running':
                $h = $this->start($name, $id);
                $this->waitStarted($id);
                $state = $h->query('state')?->getValue(0);
                if ($state !== 'waiting') {
                    throw new \RuntimeException("query answered {$state}");
                }
                $o['expected'][] = 'query state=waiting';
                $h->signal('finish');
                $settle($h);
                break;
            case 'cancel_requested':
            case 'terminated':
                $h = $this->start($name, $id);
                $this->waitStarted($id);
                $name === 'cancel_requested' ? $h->cancel() : $h->terminate('corpus terminates it');
                try {
                    $h->getResult();
                } catch (WorkflowFailedException $e) {
                    $o['expected'][] = self::describe($e);
                    return $o;
                }
                throw new \RuntimeException("the workflow completed instead of closing as {$name}");
            case 'workflow_failed_application_error':
                $h = $this->start($name, $id);
                try {
                    $h->getResult();
                } catch (WorkflowFailedException $e) {
                    $o['expected'][] = self::describe($e);
                    return $o;
                }
                throw new \RuntimeException('want an application error');
            case 'continue_as_new_3':
                $settle($this->start($name, $id, [0]));
                break;
            case 'id_reuse_allow_duplicate_failed_only':
                $policy = static fn(): WorkflowOptions => WorkflowOptions::new()
                    ->withWorkflowIdReusePolicy(IdReusePolicy::AllowDuplicateFailedOnly);
                $first = $this->start($name, $id, [true], $policy());
                try {
                    $first->getResult();
                    throw new \RuntimeException('the first run should fail');
                } catch (WorkflowFailedException) {
                }
                $second = $this->start($name, $id, [false], $policy());
                $o['expected'][] = 'the first run failed; the second started with the same ID';
                $settle($second);
                break;
            case 'id_conflict_fail':
                $policy = static fn(): WorkflowOptions => WorkflowOptions::new()
                    ->withWorkflowIdConflictPolicy(WorkflowIdConflictPolicy::Fail);
                $h = $this->start($name, $id, [], $policy());
                try {
                    $this->start($name, $id, [], $policy());
                    throw new \RuntimeException('the second start should be refused');
                } catch (WorkflowExecutionAlreadyStartedException) {
                    $o['expected'][] = 'the second start was refused: WorkflowExecutionAlreadyStarted';
                }
                $h->signal('finish');
                $settle($h);
                break;
            case 'memo':
                $settle($this->start($name, $id, [], WorkflowOptions::new()->withMemo(['initial' => 'set at start'])));
                break;
            case 'update_accepted_completed':
            case 'update_rejected':
                $h = $this->start($name, $id);
                $this->waitStarted($id);
                $by = $name === 'update_rejected' ? -1 : 2;
                try {
                    $total = $h->startUpdate(
                        UpdateOptions::new('increment', LifecycleStage::StageCompleted)->withUpdateId("{$id}-update"),
                        $by,
                    )->getResult();
                    $o['expected'][] = "update returned {$total}";
                    if ($by < 0) {
                        throw new \RuntimeException('the validator should reject a negative increment');
                    }
                } catch (WorkflowUpdateException $e) {
                    if ($by >= 0) {
                        throw $e;
                    }
                    $o['expected'][] = 'update rejected: ' . self::describe($e);
                }
                $h->signal('finish');
                $settle($h);
                break;
            default:
                $settle($this->start($name, $id));
        }
        return $o;
    }

    /** Drives the scenarios in order and writes the observations to $out. */
    public function run(string $out, ?string $only): void
    {
        $observations = [];
        foreach (Scenarios::names($only) as $name) {
            $o = $this->drive($name, Scenarios::ID_PREFIX . $name);
            \fwrite(\STDOUT, "corpus: {$name}: " . ($o['result'] ?? '') . "\n");
            $observations[] = $o;
        }
        $doc = ['namespace' => $this->namespace, 'language' => 'php', 'scenarios' => $observations];
        \file_put_contents($out, \json_encode($doc, \JSON_PRETTY_PRINT | \JSON_UNESCAPED_SLASHES | \JSON_THROW_ON_ERROR) . "\n");
    }
}

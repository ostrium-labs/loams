<?php

declare(strict_types=1);

/*
 * The corpus's PHP workflows (DX1 TSDK-php): the Go scenarios except the
 * Go-only ones (workflow_task_failure_then_fix, schedule_triggered,
 * reset_to_first_workflow_task), one class per scenario whose
 * #[WorkflowMethod] name is the scenario's name, so the starter, the worker
 * and the replayer agree on it. Activities are called untyped by the names the
 * Go and TypeScript activities have (Activities.php). A change here changes
 * the histories: regenerate the PHP corpus (generate-php.sh) in the same PR.
 *
 * PHP workflow code is a generator: each `yield` waits on a promise
 * (an activity, a timer, a condition, a child) and resumes with its value.
 * Durations are in seconds (floats allowed), as the SDK reads plain numbers.
 * The classmap autoloader (composer.json) loads every class of this file.
 */

namespace Corpus\Workflows;

use Corpus\Scenarios;
use Temporal\Activity\ActivityOptions;
use Temporal\Activity\LocalActivityOptions;
use Temporal\Api\Enums\V1\TimeoutType;
use Temporal\Common\RetryOptions;
use Temporal\DataConverter\EncodedValues;
use Temporal\Exception\Failure\ActivityFailure;
use Temporal\Exception\Failure\ApplicationFailure;
use Temporal\Exception\Failure\CanceledFailure;
use Temporal\Exception\Failure\ChildWorkflowFailure;
use Temporal\Exception\Failure\TimeoutFailure;
use Temporal\Promise;
use Temporal\Workflow;
use Temporal\Workflow\ChildWorkflowOptions;
use Temporal\Workflow\QueryMethod;
use Temporal\Workflow\SignalMethod;
use Temporal\Workflow\UpdateMethod;
use Temporal\Workflow\UpdateValidatorMethod;
use Temporal\Workflow\WorkflowInterface;
use Temporal\Workflow\WorkflowMethod;

/** Options shared by the scenarios: one attempt, 30 s to run. */
final class Opts
{
    public static function once(): ActivityOptions
    {
        return ActivityOptions::new()
            ->withStartToCloseTimeout(30)
            ->withRetryOptions(RetryOptions::new()->withMaximumAttempts(1));
    }

    /** What an activity failure says about its timeout, as the other languages' workflows report it. */
    public static function timeoutKind(\Throwable $err): string
    {
        $cause = $err instanceof ActivityFailure ? $err->getPrevious() : null;
        if ($cause instanceof TimeoutFailure) {
            return TimeoutType::name($cause->getTimeoutType());
        }
        return 'not a timeout: ' . $err->getMessage();
    }

    /** Calls an activity by name with the given options. */
    public static function call(string $name, array $args = [], ?ActivityOptions $options = null): \React\Promise\PromiseInterface
    {
        return Workflow::executeActivity($name, $args, $options ?? self::once());
    }
}

/** The `finish` signal the waiting scenarios end on. */
trait WaitsForFinish
{
    private bool $finished = false;

    #[SignalMethod(name: 'finish')]
    public function finish(): void
    {
        $this->finished = true;
    }

    private function waitFinish(): \Generator
    {
        yield Workflow::await(fn(): bool => $this->finished);
    }
}

/** The `go` signal: the scenario completes with the value it carries. */
trait AwaitsGo
{
    private ?string $value = null;

    #[SignalMethod(name: 'go')]
    public function go(string $value): void
    {
        $this->value = $value;
    }

    private function awaitGo(): \Generator
    {
        yield Workflow::await(fn(): bool => $this->value !== null);
        return $this->value;
    }
}

// --- DX1 ----------------------------------------------------------------------

#[WorkflowInterface]
final class Hello
{
    #[WorkflowMethod(name: 'hello')]
    public function run(string $name)
    {
        return yield Opts::call('greet', [$name]);
    }
}

#[WorkflowInterface]
final class TwoActivitiesSequential
{
    #[WorkflowMethod(name: 'two_activities_sequential')]
    public function run()
    {
        $a = yield Opts::call('greet', ['one']);
        $b = yield Opts::call('greet', ['two']);
        return "{$a}, {$b}";
    }
}

#[WorkflowInterface]
final class ActivityParallelFanout10
{
    #[WorkflowMethod(name: 'activity_parallel_fanout_10')]
    public function run()
    {
        $all = [];
        for ($i = 0; $i < 10; $i++) {
            $all[] = Opts::call('greet', ["n{$i}"]);
        }
        return yield Promise::all($all);
    }
}

#[WorkflowInterface]
final class SignalThenComplete
{
    use AwaitsGo;

    #[WorkflowMethod(name: 'signal_then_complete')]
    public function run()
    {
        $v = yield from $this->awaitGo();
        return "got {$v}";
    }
}

#[WorkflowInterface]
final class QueryWhileRunning
{
    use WaitsForFinish;

    private string $state = 'waiting';

    #[QueryMethod(name: 'state')]
    public function state(): string
    {
        return $this->state;
    }

    #[WorkflowMethod(name: 'query_while_running')]
    public function run()
    {
        yield from $this->waitFinish();
        $this->state = 'done';
        return $this->state;
    }
}

#[WorkflowInterface]
final class CancelRequested
{
    #[WorkflowMethod(name: 'cancel_requested')]
    public function run()
    {
        // The client's cancel fails this timer with CanceledFailure, which
        // closes the workflow as cancelled.
        yield Workflow::timer(3600);
    }
}

#[WorkflowInterface]
final class Terminated
{
    use WaitsForFinish;

    #[WorkflowMethod(name: 'terminated')]
    public function run()
    {
        yield from $this->waitFinish();
    }
}

#[WorkflowInterface]
final class WorkflowFailedApplicationError
{
    #[WorkflowMethod(name: 'workflow_failed_application_error')]
    public function run()
    {
        throw new ApplicationFailure('boom', 'CorpusBoom', true, EncodedValues::fromValues(['detail']));
    }
}

#[WorkflowInterface]
final class ActivityHeartbeatDetails
{
    #[WorkflowMethod(name: 'activity_heartbeat_details')]
    public function run()
    {
        return yield Opts::call('heartbeat_thrice', [], Opts::once()->withHeartbeatTimeout(10));
    }
}

// --- DX2 ----------------------------------------------------------------------

#[WorkflowInterface]
final class TimerShort
{
    #[WorkflowMethod(name: 'timer_short')]
    public function run()
    {
        yield Workflow::timer(1);
    }
}

#[WorkflowInterface]
final class TimerCancelled
{
    #[WorkflowMethod(name: 'timer_cancelled')]
    public function run()
    {
        $long = Workflow::async(static function () {
            yield Workflow::timer(3600);
        });
        yield Workflow::timer(0.1);
        $long->cancel();
        try {
            yield $long;
        } catch (CanceledFailure) {
            return 'cancelled';
        }
        throw new \RuntimeException('the long timer was not cancelled');
    }
}

#[WorkflowInterface]
final class ActivityRetryBackoff3Attempts
{
    #[WorkflowMethod(name: 'activity_retry_backoff_3_attempts')]
    public function run()
    {
        return yield Opts::call('fail_twice', [], ActivityOptions::new()
            ->withStartToCloseTimeout(10)
            ->withRetryOptions(RetryOptions::new()
                ->withInitialInterval(1)
                ->withBackoffCoefficient(2.0)
                ->withMaximumAttempts(3)));
    }
}

#[WorkflowInterface]
final class ActivityStartToCloseTimeout
{
    #[WorkflowMethod(name: 'activity_start_to_close_timeout')]
    public function run()
    {
        try {
            yield Opts::call('sleep_for', [5000], ActivityOptions::new()
                ->withStartToCloseTimeout(1)
                ->withRetryOptions(RetryOptions::new()->withMaximumAttempts(1)));
            return 'no timeout';
        } catch (\Throwable $e) {
            return Opts::timeoutKind($e);
        }
    }
}

#[WorkflowInterface]
final class ActivityScheduleToStartTimeout
{
    #[WorkflowMethod(name: 'activity_schedule_to_start_timeout')]
    public function run()
    {
        try {
            yield Opts::call('greet', ['nobody'], ActivityOptions::new()
                ->withTaskQueue('corpus-no-worker')
                ->withScheduleToStartTimeout(1)
                ->withStartToCloseTimeout(1)
                ->withRetryOptions(RetryOptions::new()->withMaximumAttempts(1)));
            return 'no timeout';
        } catch (\Throwable $e) {
            return Opts::timeoutKind($e);
        }
    }
}

#[WorkflowInterface]
final class ActivityHeartbeatTimeout
{
    #[WorkflowMethod(name: 'activity_heartbeat_timeout')]
    public function run()
    {
        try {
            yield Opts::call('sleep_for', [5000], ActivityOptions::new()
                ->withStartToCloseTimeout(30)
                ->withHeartbeatTimeout(1)
                ->withRetryOptions(RetryOptions::new()->withMaximumAttempts(1)));
            return 'no timeout';
        } catch (\Throwable $e) {
            return Opts::timeoutKind($e);
        }
    }
}

#[WorkflowInterface]
final class SignalWithStart
{
    use AwaitsGo;

    #[WorkflowMethod(name: 'signal_with_start')]
    public function run()
    {
        $v = yield from $this->awaitGo();
        return "started with {$v}";
    }
}

#[WorkflowInterface]
final class ChildGreeter
{
    #[WorkflowMethod(name: 'child_greeter')]
    public function run(string $name)
    {
        return yield Opts::call('greet', [$name]);
    }
}

#[WorkflowInterface]
final class ChildSleeper
{
    #[WorkflowMethod(name: 'child_sleeper')]
    public function run()
    {
        yield Workflow::timer(3600);
    }
}

/**
 * The child's ID is corpus-php-<scenario>-child, the parent's ID plus
 * `-child` as in the other languages, but built from the parent's workflow
 * type rather than read from its ID: RoadRunner's replay (replay.php) runs
 * the history under the Go replayer's own workflow ID, and a child ID read
 * from it would not match the history's.
 */
final class Child
{
    public static function options(): ChildWorkflowOptions
    {
        return ChildWorkflowOptions::new()
            ->withWorkflowId(Scenarios::ID_PREFIX . Workflow::getInfo()->type->name . '-child')
            ->withTaskQueue(Scenarios::TASK_QUEUE);
    }
}

#[WorkflowInterface]
final class ChildWorkflow
{
    #[WorkflowMethod(name: 'child_workflow')]
    public function run()
    {
        return yield Workflow::executeChildWorkflow('child_greeter', ['child'], Child::options());
    }
}

#[WorkflowInterface]
final class ChildWorkflowCancelled
{
    #[WorkflowMethod(name: 'child_workflow_cancelled')]
    public function run()
    {
        $child = Workflow::newUntypedChildWorkflowStub('child_sleeper', Child::options());
        $scope = Workflow::async(static function () use ($child) {
            yield $child->start();
            return yield $child->getResult();
        });
        // Cancel once the child has started, as the other languages do.
        yield $child->getExecution();
        $scope->cancel();
        try {
            yield $scope;
        } catch (CanceledFailure|ChildWorkflowFailure) {
            return 'child cancelled';
        }
        throw new \RuntimeException('the child was not cancelled');
    }
}

#[WorkflowInterface]
final class ContinueAsNew3
{
    #[WorkflowMethod(name: 'continue_as_new_3')]
    public function run(int $n)
    {
        if ($n < 3) {
            return yield Workflow::continueAsNew('continue_as_new_3', [$n + 1]);
        }
        return $n;
    }
}

#[WorkflowInterface]
final class IdReuseAllowDuplicateFailedOnly
{
    #[WorkflowMethod(name: 'id_reuse_allow_duplicate_failed_only')]
    public function run(bool $fail)
    {
        if ($fail) {
            throw new ApplicationFailure('first run fails', 'CorpusFirstRun', true);
        }
        return 'second run';
    }
}

#[WorkflowInterface]
final class IdConflictFail
{
    use WaitsForFinish;

    #[WorkflowMethod(name: 'id_conflict_fail')]
    public function run()
    {
        yield from $this->waitFinish();
    }
}

#[WorkflowInterface]
final class LocalActivity
{
    #[WorkflowMethod(name: 'local_activity')]
    public function run()
    {
        return yield Workflow::executeActivity(
            'greet',
            ['local'],
            LocalActivityOptions::new()->withStartToCloseTimeout(10),
        );
    }
}

#[WorkflowInterface]
final class SideEffectAndVersionMarker
{
    #[WorkflowMethod(name: 'side_effect_and_version_marker')]
    public function run()
    {
        // As Go: a version marker, then a side-effect marker.
        $v = yield Workflow::getVersion('corpus-change', Workflow::DEFAULT_VERSION, 1);
        $n = yield Workflow::sideEffect(static fn(): int => 42);
        return "version {$v} side effect {$n}";
    }
}

// --- DX3 ----------------------------------------------------------------------

#[WorkflowInterface]
final class SearchAttributesUpsert
{
    #[WorkflowMethod(name: 'search_attributes_upsert')]
    public function run()
    {
        Workflow::upsertSearchAttributes(['CorpusKeyword' => 'first']);
        yield Workflow::timer(0.1);
        Workflow::upsertSearchAttributes(['CorpusKeyword' => 'second']);
    }
}

#[WorkflowInterface]
final class Memo
{
    #[WorkflowMethod(name: 'memo')]
    public function run()
    {
        // Nothing to wait on: a plain method (the SDK runs both kinds).
        Workflow::upsertMemo(['step' => 'upserted', 'n' => 2]);
    }
}

#[WorkflowInterface]
final class LargePayload1mb
{
    #[WorkflowMethod(name: 'large_payload_1mb')]
    public function run()
    {
        $s = yield Opts::call('large', [1 << 20]);
        return \strlen($s);
    }
}

// --- DX4 ----------------------------------------------------------------------

/** The `increment` update with its validator, then `finish`. */
trait Updates
{
    use WaitsForFinish;

    private int $total = 0;

    #[UpdateMethod(name: 'increment')]
    public function increment(int $by): int
    {
        $this->total += $by;
        return $this->total;
    }

    #[UpdateValidatorMethod(forUpdate: 'increment')]
    public function validateIncrement(int $by): void
    {
        if ($by < 0) {
            throw new \InvalidArgumentException('increment must not be negative');
        }
    }

    private function updates(): \Generator
    {
        yield from $this->waitFinish();
        return "total {$this->total}";
    }
}

#[WorkflowInterface]
final class UpdateAcceptedCompleted
{
    use Updates;

    #[WorkflowMethod(name: 'update_accepted_completed')]
    public function run()
    {
        return yield from $this->updates();
    }
}

#[WorkflowInterface]
final class UpdateRejected
{
    use Updates;

    #[WorkflowMethod(name: 'update_rejected')]
    public function run()
    {
        return yield from $this->updates();
    }
}

/** Every workflow class the worker and the replayer register. */
final class All
{
    /** @return list<class-string> */
    public static function classes(): array
    {
        return [
            Hello::class,
            TwoActivitiesSequential::class,
            ActivityParallelFanout10::class,
            SignalThenComplete::class,
            QueryWhileRunning::class,
            CancelRequested::class,
            Terminated::class,
            WorkflowFailedApplicationError::class,
            ActivityHeartbeatDetails::class,
            TimerShort::class,
            TimerCancelled::class,
            ActivityRetryBackoff3Attempts::class,
            ActivityStartToCloseTimeout::class,
            ActivityScheduleToStartTimeout::class,
            ActivityHeartbeatTimeout::class,
            SignalWithStart::class,
            ChildGreeter::class,
            ChildSleeper::class,
            ChildWorkflow::class,
            ChildWorkflowCancelled::class,
            ContinueAsNew3::class,
            IdReuseAllowDuplicateFailedOnly::class,
            IdConflictFail::class,
            LocalActivity::class,
            SideEffectAndVersionMarker::class,
            SearchAttributesUpsert::class,
            Memo::class,
            LargePayload1mb::class,
            UpdateAcceptedCompleted::class,
            UpdateRejected::class,
        ];
    }
}

<?php

declare(strict_types=1);

namespace Corpus;

use Temporal\Activity;
use Temporal\Activity\ActivityInterface;
use Temporal\Activity\ActivityMethod;
use Temporal\Exception\Failure\ApplicationFailure;

/**
 * The corpus's PHP activities, named as the Go and TypeScript ones. RoadRunner
 * runs them in its activity worker pool (.rr.yaml).
 */
#[ActivityInterface(prefix: '')]
final class Activities
{
    #[ActivityMethod(name: 'greet')]
    public function greet(string $name): string
    {
        return "hello {$name}";
    }

    #[ActivityMethod(name: 'heartbeat_thrice')]
    public function heartbeatThrice(): int
    {
        for ($i = 1; $i <= 3; $i++) {
            Activity::heartbeat($i);
        }
        return 3;
    }

    #[ActivityMethod(name: 'fail_twice')]
    public function failTwice(): int
    {
        $attempt = Activity::getInfo()->attempt;
        if ($attempt < 3) {
            throw new ApplicationFailure("attempt {$attempt} fails", 'Transient', false);
        }
        return $attempt;
    }

    /** Sleeps without heartbeating (the start-to-close and heartbeat timeouts). */
    #[ActivityMethod(name: 'sleep_for')]
    public function sleepFor(int $ms): void
    {
        \usleep($ms * 1000);
    }

    #[ActivityMethod(name: 'large')]
    public function large(int $n): string
    {
        return \str_repeat('x', $n);
    }
}

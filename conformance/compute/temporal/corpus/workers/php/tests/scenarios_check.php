<?php

declare(strict_types=1);

/*
 * The PHP corpus worker's offline checks (DX1 TSDK-php), no server needed:
 *
 *   - Scenarios::ALL lists scenarios.json's non-Go-only scenarios in order
 *     (manifest.py check does the same from Python);
 *   - every scenario, and the two child workflows, has exactly one workflow
 *     class whose #[WorkflowMethod] name is that scenario, among the classes
 *     worker.php registers;
 *   - the activities carry the Go and TypeScript activity names.
 *
 *   php tests/scenarios_check.php     (corpus-php.sh check)
 */

require __DIR__ . '/../vendor/autoload.php';

use Corpus\Activities;
use Corpus\Scenarios;
use Corpus\Workflows\All;
use Temporal\Activity\ActivityMethod;
use Temporal\Workflow\WorkflowInterface;
use Temporal\Workflow\WorkflowMethod;

$problems = [];
$check = static function (bool $ok, string $what) use (&$problems): void {
    echo ($ok ? 'ok   ' : 'FAIL '), $what, "\n";
    $ok or $problems[] = $what;
};

$doc = \json_decode((string) \file_get_contents(__DIR__ . '/../../../scenarios.json'), true, flags: \JSON_THROW_ON_ERROR);
$want = [];
foreach ($doc['scenarios'] as $s) {
    if (!$s['go_only']) {
        $want[] = [$s['name'], $s['phase']];
    }
}
$check(Scenarios::ALL === $want, 'Scenarios::ALL equals scenarios.json without the Go-only scenarios, in order');

$types = [];
foreach (All::classes() as $class) {
    $r = new \ReflectionClass($class);
    $check($r->getAttributes(WorkflowInterface::class) !== [], "{$class} is a #[WorkflowInterface]");
    $methods = [];
    foreach ($r->getMethods() as $m) {
        foreach ($m->getAttributes(WorkflowMethod::class) as $a) {
            $methods[] = $a->newInstance()->name;
        }
    }
    $check(\count($methods) === 1, "{$class} has one #[WorkflowMethod]");
    $types[] = $methods[0] ?? '';
}
$check(\count($types) === \count(\array_unique($types)), 'workflow types are unique');
foreach ([...Scenarios::names(), 'child_greeter', 'child_sleeper'] as $name) {
    $check(\in_array($name, $types, true), "a workflow is registered as {$name}");
}

$activities = [];
foreach ((new \ReflectionClass(Activities::class))->getMethods() as $m) {
    foreach ($m->getAttributes(ActivityMethod::class) as $a) {
        $activities[] = $a->newInstance()->name;
    }
}
\sort($activities);
$check($activities === ['fail_twice', 'greet', 'heartbeat_thrice', 'large', 'sleep_for'], 'activities are named as Go\'s and TypeScript\'s');

echo $problems === [] ? "scenarios_check: ok\n" : 'scenarios_check: ' . \count($problems) . " problem(s)\n";
exit($problems === [] ? 0 : 1);

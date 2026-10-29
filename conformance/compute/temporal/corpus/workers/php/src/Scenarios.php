<?php

declare(strict_types=1);

namespace Corpus;

/**
 * The PHP corpus's scenarios, in scenarios.json's order without the Go-only
 * ones (manifest.py check compares this list with scenarios.json, one
 * `['name', 'DXn'],` per line).
 */
final class Scenarios
{
    public const TASK_QUEUE = 'corpus-php';
    public const ID_PREFIX = 'corpus-php-';

    /** @var list<array{0: string, 1: string}> */
    public const ALL = [
        ['hello', 'DX1'],
        ['two_activities_sequential', 'DX1'],
        ['activity_parallel_fanout_10', 'DX1'],
        ['signal_then_complete', 'DX1'],
        ['query_while_running', 'DX1'],
        ['cancel_requested', 'DX1'],
        ['terminated', 'DX1'],
        ['workflow_failed_application_error', 'DX1'],
        ['activity_heartbeat_details', 'DX1'],
        ['timer_short', 'DX2'],
        ['timer_cancelled', 'DX2'],
        ['activity_retry_backoff_3_attempts', 'DX2'],
        ['activity_start_to_close_timeout', 'DX2'],
        ['activity_schedule_to_start_timeout', 'DX2'],
        ['activity_heartbeat_timeout', 'DX2'],
        ['signal_with_start', 'DX2'],
        ['child_workflow', 'DX2'],
        ['child_workflow_cancelled', 'DX2'],
        ['continue_as_new_3', 'DX2'],
        ['id_reuse_allow_duplicate_failed_only', 'DX2'],
        ['id_conflict_fail', 'DX2'],
        ['local_activity', 'DX2'],
        ['side_effect_and_version_marker', 'DX2'],
        ['search_attributes_upsert', 'DX3'],
        ['memo', 'DX3'],
        ['large_payload_1mb', 'DX3'],
        ['update_accepted_completed', 'DX4'],
        ['update_rejected', 'DX4'],
    ];

    /** @return list<string> */
    public static function names(?string $only = null): array
    {
        $want = $only === null || $only === '' ? null : \explode(',', $only);
        $out = [];
        foreach (self::ALL as [$name]) {
            if ($want === null || \in_array($name, $want, true)) {
                $out[] = $name;
            }
        }
        return $out;
    }
}

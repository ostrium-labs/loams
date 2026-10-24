-- HE0 Task 10: 20 representative Tier 0 queries planned into DataFusion
-- logical plans. Written by hand for this spike over the stub schemas in
-- src/ctx.rs (events, traces, observations, scores, daily); no application
-- source and no ClickBench text.

-- @id q01_daily_counts
-- @features alias_in_group_by,function:toStartOfDay,function:uniq,count
SELECT toStartOfDay(ts) AS day, count() AS events, uniq(profile_id) AS users FROM events WHERE project_id = 'p1' AND ts >= now() - INTERVAL 7 DAY GROUP BY day ORDER BY day

-- @id q02_if_combinators
-- @features combinator:If
SELECT name, countIf(duration > 1000) AS slow, sumIf(v, path LIKE '/api/%') AS api_v, uniqIf(profile_id, country = 'DE') AS de_users FROM events GROUP BY name

-- @id q03_parametric_quantiles
-- @features parametric,function:quantile,function:quantiles
SELECT name, quantile(0.95)(duration) AS p95, quantiles(0.5, 0.9, 0.99)(duration) AS ps FROM events GROUP BY name

-- @id q04_final
-- @features final
SELECT id, name, timestamp FROM traces FINAL WHERE project_id = 'p1' ORDER BY timestamp DESC LIMIT 50

-- @id q05_limit_by
-- @features limit_by
SELECT trace_id, id, start_time FROM observations WHERE project_id = 'p1' ORDER BY trace_id, start_time DESC LIMIT 3 BY trace_id

-- @id q06_with_totals
-- @features with_totals
SELECT country, count() AS c, avg(duration) AS d FROM events GROUP BY country WITH TOTALS ORDER BY c DESC

-- @id q07_array_join
-- @features array_join
SELECT tag, count() AS c FROM traces ARRAY JOIN tags AS tag WHERE project_id = 'p1' GROUP BY tag ORDER BY c DESC

-- @id q08_left_array_join_map
-- @features array_join:left,function:mapKeys
SELECT id, k FROM traces LEFT ARRAY JOIN mapKeys(metadata) AS k WHERE project_id = 'p1'

-- @id q09_array_join_function
-- @features function:arrayJoin
SELECT arrayJoin(tags) AS tag, id FROM traces WHERE project_id = 'p1'

-- @id q10_map_and_json
-- @features map_subscript
SELECT metadata['environment'] AS env, count() AS c FROM traces WHERE metadata['release'] != '' GROUP BY env

-- @id q11_window_funnel
-- @features parametric,function:windowFunnel
SELECT level, count() AS users FROM (SELECT profile_id, windowFunnel(86400)(toDateTime(ts), name = 'view', name = 'cart', name = 'buy') AS level FROM events WHERE project_id = 'p1' GROUP BY profile_id) GROUP BY level ORDER BY level

-- @id q12_argmax_dedup
-- @features function:argMax
SELECT id, argMax(name, event_ts) AS latest_name, argMax(is_deleted, event_ts) AS deleted FROM traces WHERE project_id = 'p1' GROUP BY id HAVING deleted = 0

-- @id q13_cte_left_join
-- @features cte,join
WITH obs AS (SELECT trace_id, count() AS n, max(end_time) AS last_end FROM observations WHERE project_id = 'p1' GROUP BY trace_id) SELECT t.id, t.name, o.n, o.last_end FROM traces AS t LEFT JOIN obs AS o ON o.trace_id = t.id WHERE t.project_id = 'p1' ORDER BY t.timestamp DESC LIMIT 100

-- @id q14_window_functions
-- @features window,function:lagInFrame
SELECT session_id, ts, row_number() OVER (PARTITION BY session_id ORDER BY ts) AS rn, lagInFrame(ts) OVER (PARTITION BY session_id ORDER BY ts ROWS BETWEEN UNBOUNDED PRECEDING AND UNBOUNDED FOLLOWING) AS prev FROM events WHERE project_id = 'p1'

-- @id q15_prewhere
-- @features prewhere
SELECT id, name, ts FROM events PREWHERE project_id = 'p1' WHERE name = 'click' ORDER BY ts DESC LIMIT 20

-- @id q16_state_merge
-- @features combinator:Merge
SELECT day, uniqMerge(users) AS users, sum(events) AS events FROM daily WHERE project_id = 'p1' GROUP BY day ORDER BY day

-- @id q17_alias_scoping
-- @features alias_in_where,alias_reuse
SELECT toDate(ts) AS d, duration / 1000 AS secs, secs * 2 AS double_secs FROM events WHERE d >= today() - 7 AND secs > 1

-- @id q18_lambdas
-- @features lambda,function:arrayFilter,function:arrayMap,function:has
SELECT id, arrayFilter(x -> x != '', tags) AS non_empty, arrayMap(x -> upper(x), tags) AS up, has(tags, 'prod') AS is_prod FROM traces WHERE project_id = 'p1'

-- @id q19_params_settings_format
-- @features query_params,settings,format
SELECT name, count() AS c FROM events WHERE project_id = {projectId: String} AND ts >= {from: DateTime64(3)} GROUP BY name ORDER BY c DESC LIMIT {limit: UInt32} SETTINGS max_threads = 4 FORMAT JSONEachRow

-- @id q20_casts_buckets_conditionals
-- @features cast:ch_types,function:toStartOfInterval,function:dateDiff,function:multiIf
SELECT toStartOfInterval(start_time, INTERVAL 15 minute) AS bucket, multiIf(dateDiff('millisecond', start_time, end_time) < 100, 'fast', dateDiff('millisecond', start_time, end_time) < 1000, 'ok', 'slow') AS speed, CAST(type AS LowCardinality(String)) AS t, CAST(count() AS UInt64) AS n FROM observations WHERE project_id = 'p1' AND end_time IS NOT NULL GROUP BY bucket, speed, t ORDER BY bucket

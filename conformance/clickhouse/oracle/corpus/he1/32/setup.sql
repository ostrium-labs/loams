-- HE1 Task 32: arrays, maps, tuples and lambdas. Shapes from the mined
-- uses (Langfuse's metadata names/values and usage maps, OpenPanel's event
-- paths, SigNoz's attribute maps, Plausible's custom props), plus empty
-- arrays, NULL elements, nested arrays and LowCardinality map keys.
DROP TABLE IF EXISTS t32;
CREATE TABLE t32 (
    id UInt32,
    tags Array(String),
    names Array(String),
    vals Array(String),
    nums Array(UInt8),
    nn Array(Nullable(UInt8)),
    nested Array(Array(UInt16)),
    lcarr Array(LowCardinality(String)),
    usage Map(LowCardinality(String), UInt64),
    attrs Map(String, String),
    pair Tuple(UInt8, String),
    named Tuple(a UInt8, b String)
) ENGINE = MergeTree ORDER BY id;
INSERT INTO t32 VALUES
    (1, ['b', 'a', 'b'], ['env', 'step'], ['prod', 'two'], [10, 20, 30], [1, NULL, 3], [[1, 2], [], [3]], ['x', 'y', 'x'], {'input': 10, 'output': 5, 'input_cached': 2}, {'service.name': 'api', 'k': ''}, (7, 'x'), (1, 'p')),
    (2, [], [], [], [], [], [], [], {}, {}, (0, ''), (0, '')),
    (3, ['z'], ['env'], ['dev'], [5], [NULL], [[]], ['y'], {'output': 1}, {'k': 'v'}, (255, 'y'), (2, 'q'));

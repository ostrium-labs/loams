-- expect: ok
-- HE1 R9.62: a one-node House reads clusterAllReplicas of a system table locally.
SELECT * FROM clusterAllReplicas('default', system.one)

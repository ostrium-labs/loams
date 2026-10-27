-- expect: 344
-- HE1 R9.62: clusterAllReplicas of anything but a system table stays refused.
SELECT * FROM clusterAllReplicas('default', default.t)

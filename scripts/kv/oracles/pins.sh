# The KV oracles' pins (KV1 plan KV0 Task 0, ruling T0-6). Sourced.
# shellcheck shell=bash
VALKEY_REPO=valkey-io/valkey
VALKEY_TAG=9.1.2
VALKEY_COMMIT=7f1dffedff6de73058b2c2a389422b6ecd56c8fb
VALKEY_IMAGE=docker.io/valkey/valkey:9.1.2@sha256:418652cfb58ef879d4978c33553735d7147016032d5aefaa14c828e611eb9dfd
CASSANDRA_IMAGE=docker.io/library/cassandra:5.0.9@sha256:3ea9333712c5692f3fe91d6c4dcc262df9e36338c4fdd1bc00a5e53f1137a1a0
TIDIS_REPO=tidb-incubator/tidis
TIDIS_COMMIT=555856206f248737b12d3f148b10f08a604bc821
TITAN_REPO=distributedio/titan
TITAN_COMMIT=97868f5d24e0c13621658fbcb712064fec59e8be
# The token Titan's autotest authenticates with (tools/autotest/auto.go).
TITAN_AUTOTEST_TOKEN=test-1542098935-1-7ca41bda4efc2a1889c04e

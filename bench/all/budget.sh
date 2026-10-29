# shellcheck shell=bash
# The core and memory budget of the BQ1 combined run (README.md explains it).
# Sourced by engines.sh and run.sh. Laptop: Intel Core Ultra 5 225H, 14 cores
# without SMT: 0-3 P-cores (4.9 GHz), 4-11 E-cores (4.4 GHz), 12-13
# low-power E-cores (2.5 GHz); 15 GB RAM.
#
# Every server has its own core. Each network engine's load generator has
# its own core. The four in-process engines (the loams-kv memory store,
# Grafeo, the meter gate and sink, git-on-S3 on a file store) are both server
# and load, so they share the two low-power cores in pairs.

# Servers that engines.sh starts.
: "${TIKV_CORES:=0}"      # TiKV: the sql engine's store, and the kv engine's TiKV op
: "${TIDB_CORES:=1}"      # TiDB (keyspace mode)
: "${PD_CORES:=1}"        # PD shares TiDB's core: it only hands out timestamps here
: "${PG_CORES:=2}"        # Postgres 17.10 (stock compute)
: "${RESONATE_CORES:=5}"  # Resonate on SQLite
: "${GATEWAY_CORES:=6}"   # loams-gateway
# Servers that start inside their engine's native-load.sh.
: "${HOUSE_CORES:=3}"     # the House (loams-fabric + workers)
: "${IGGY_CORES:=4}"      # iggy-server
# Load generators.
: "${SQL_LOAD:=7}"        # MySQL wire to TiDB, and the kv engine's TiKV op
: "${PG_LOAD:=8}"
: "${HOUSE_LOAD:=9}"
: "${IGGY_LOAD:=10}"
: "${DURABLE_LOAD:=11}"   # Resonate and the gateway share this core
: "${GATEWAY_LOAD:=11}"
# In-process engines (server and load in one process).
: "${KV_CORES:=12}"
: "${GRAPH_CORES:=12}"
: "${METER_CORES:=13}"
: "${GIT_CORES:=13}"

# Memory caps (container limits; host processes are sized by their data).
: "${TIKV_MEM:=1400m}" "${TIKV_MEM_LIMIT:=1100MB}" "${TIKV_BLOCK_CACHE:=256MB}"
: "${TIDB_MEM:=1200m}" "${PD_MEM:=512m}" "${PG_MEM:=768m}"

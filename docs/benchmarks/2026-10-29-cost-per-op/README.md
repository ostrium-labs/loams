# What a million operations cost on Loams

Lane COST, 2026-10-29. Script: [`bench/cost/compute.py`](../../../bench/cost/compute.py).
Prices and their sources: [`bench/cost/prices.toml`](../../../bench/cost/prices.toml).
Throughput: the BQ1 combined run ([`../2026-10-28-1b-qps/combined.md`](../2026-10-28-1b-qps/combined.md))
and the Confluent Cloud run ([`../confluent-cloud-2026-10.md`](../confluent-cloud-2026-10.md)).

**Status: the throughput is measured, on one 14-core laptop. Every dollar
figure is projected: a measured throughput multiplied by a price that is
published or assumed.** Nothing here is a bill for running Loams in
production, except the Confluent spend, which is our own bill (an upper
bound until the Costs API reconciles it).

## Labels

Every number carries one of these labels, in its column header or next to it:

| Label | Meaning |
|---|---|
| **[M] measured** | Read from our result files, bill or ledger. |
| **[published]** | Read from a vendor's or agency's published price, with the date it was read. |
| **[A] assumption** | Chosen by us. The sensitivity table varies it. |
| **[P] projected** | Computed from measured and published or assumed inputs. Every $ per 1M ops is [P]. |

## Headline

On-demand cloud prices, pipelined (depth 16), 1 vCPU per laptop core unless noted.

- **The whole BQ1 mix (all eleven slots at once) costs about $0.00014 per
  million operations [P]**, or 14 cents per billion. That is 89,360 ops/s per
  busy core [M] at $0.0446 per vCPU-hour (AWS c7i.large on-demand
  [published]). On a 3-year savings plan it is $0.000061 [P]; on owned
  hardware of the same CPU, $0.000011 [P]. At depth 1 the mix costs $0.00023
  [P] on demand.
- **Per engine, the spread is about 1,000×.** The meter gate is the cheapest
  at $0.000016 per million checks [P]. Resonate on SQLite is the costliest
  engine that completed, at $0.018 per million promise creations [P], because
  each create is its own SQLite commit. The House is $0.53 per million point
  queries at depth 1 [P] and has no depth-16 number.
- **Mapping a laptop core to two vCPUs (one whole c7i physical core) doubles
  every cloud figure.** The sensitivity table shows both.
- **Confluent Dedicated costs about $0.0087 per million 100-byte records [P]**
  (an assumed $4.00 per CKU-hour [A] plus $0.05 per GB in [published], at the
  measured 316,803 records/s per CKU [M]), and **$0.068 per GB at 1 KB
  records [P]**. Our whole Confluent benchmark came to **$0.38 per GB
  ingested [M]** ($1,211.05 upper bound over 3,177 GB), because it includes
  generators, idle hours and smaller cluster types.
- **Iggy, measured per core on the laptop, comes out about 50× cheaper per
  100-byte message than Confluent [P]**, after we triple its server cost to
  stand in for replication factor 3 [A]. Counting Confluent's CKU charge
  only, without the data-in fee, the gap is about 20× [P]. This comparison is
  not like for like, and the caveats below are larger than they look.

## Method

For each engine and depth:

```text
ops per busy core-second  = mean QPS / mean busy cores                   [M]
$ per busy core-hour      = $ per vCPU-hour × vCPUs per laptop core      [published × A]
                          or (hardware ÷ 3 years ÷ 14 cores) + (watts × $/kWh ÷ 14)   [published + A]
$ per 1M ops              = $ per busy core-hour × 1,000,000 / (ops per busy core-second × 3,600)   [P]
```

- **QPS and busy cores** are the means of runs A and B of the BQ1 combined
  run. "Busy cores" is the measured busy time of an engine's server cores plus
  its share of its load generator's cores, from `/proc/stat`, with every
  process pinned to disjoint cores. The numbers below reproduce the "QPS per
  busy core" column of the combined report.
- **Depth 1** is one operation in flight per connection (or per batch).
  **Depth 16** is the pipelined or batched case; what "16" means per engine
  is in the combined report's first table.
- **A batch of N counts as N operations.** A Postgres or TiDB point select,
  an Iggy message, a meter check and a Resonate promise are each one
  operation. They are not the same amount of work, so compare engines with
  care.
- **Cloud prices are per vCPU-hour.** A c7i vCPU is one hyperthread. The
  laptop's Intel Core Ultra 5 225H has no SMT, so each "busy core" is one
  physical core: 4 P-cores (4.9 GHz), 8 E-cores (4.4 GHz) and 2 low-power
  E-cores (2.5 GHz). We map one laptop core to one vCPU by default [A] and
  show two in the sensitivity table.
- **Owned hardware** is a mini PC with the same CPU, so the measured
  throughput carries over without a CPU conversion. It is amortised straight
  line over 3 years [A] of 24/7 use [A], plus power at the US commercial rate.

### Inputs the script read

<!-- cost:inputs -->
| Input | What the script read [M] | File |
|---|---|---|
| BQ1 combined runs | 2 runs at depth 1, 2 at depth 16; Intel Core Ultra 5 225H; commit c4760980c | bench/results/bq1/all/*.json |
| Confluent Dedicated (aws-d4) | phase A 251.1 MB/s, phase B 1,267,212 rec/s on 4 CKU | bench/confluent/results/2026-10-28/dedicated/loams-cfb-aws-d4-series-1300.json |
| Confluent burst (aws-x2) | peak minute 17:52Z: 4,339,675 rec/s on 4 CKU | bench/confluent/results/2026-10-28/burst/cal96-loams-cfb-aws-x2-series.json |
<!-- /cost:inputs -->

### The three reference prices

All three were read on 2026-10-29.

<!-- cost:prices -->
| Reference | $ per busy core-hour | Basis | Source |
|---|---:|---|---|
| AWS EC2 c7i.large on-demand | $0.0446 [published] | $0.08925 / h ÷ 2 vCPU × 1 vCPU per core | AWS Price List (pricing calculator feed ec2-ondemand-without-sec-sel, US East (N. Virginia), Linux), publication 2026-10-28T18:03:52Z; https://aws.amazon.com/ec2/pricing/on-demand/ |
| AWS EC2 Instance Savings Plan, c7i, 3 years, no upfront | $0.0196 [published] | $0.03929 / h ÷ 2 vCPU × 1 vCPU per core | AWS Price List API, AWSComputeSavingsPlan us-east-1 offer file, publication 2026-10-28T18:02:49Z, rate code D2YVZHPR4MTDUCJY.4TFFGXJMCK23ZSWR; https://aws.amazon.com/savingsplans/compute-pricing/ |
| Owned: Gigabyte BRIX GB-BRU5-225H (Core Ultra 5 225H), 3 years + power | $0.00338 [published + assumption] | ($612.93 + $250) ÷ (3 y × 8,760 h) ÷ 14 cores + 100 W × $0.1447/kWh ÷ 14 | AVADirect product 19197029, GIGABYTE BRIX GB-BRU5-225H barebone, $612.93; https://www.avadirect.com/BRIX-GB-BRU5-225H-Intel-Core-Ultra-5-225H-2x-DDR5-CSO-DIMM-2x-M-2-NVMe-SSD-Intel-Arc-GPU-Mini-PC-Barebone/Product/19197029; US EIA, Electric Power Monthly table 5.6.A, commercial sector, August 2026 (preliminary), 14.47 cents/kWh; https://www.eia.gov/electricity/monthly/epm_table_grapher.php?t=epmt_5_6_a |
<!-- /cost:prices -->

Notes on the prices:

- **On-demand:** c7i.large in us-east-1 is the reference x86 compute
  instance. c8i.large is 5 % dearer and is in the sensitivity table.
- **3-year:** the EC2 Instance Savings Plan for the c7i family, no upfront,
  is AWS's current 3-year commitment for this family. The all-upfront plan
  ($0.0342/h) is in the sensitivity table. We did not read Standard
  Reserved Instance prices; the Savings Plan offer file is the one we read.
- **Owned:** the barebone's price is a retail listing [published]. The RAM
  and SSD ($250) are an assumption [A], because DRAM prices moved a lot in
  2026. The 100 W is the power supply's rating [published], an upper bound on
  what the box draws. The CPU's base power is 28 W. The owned figure leaves
  out rack space, cooling overhead (PUE), networking, spares and people. It
  is the floor for hardware you already run, not a total cost of ownership.

## Cost per million operations, per engine

[M] for throughput; [P] for every dollar figure. On-demand and 3-year plan
use 1 vCPU per busy core [A]. "Owned" is the 225H mini PC over 3 years at
100 W.

<!-- cost:engines -->
| Engine | Operation | ops/s per busy core, d1 [M] | ops/s per busy core, d16 [M] | On-demand d1 [P] | On-demand d16 [P] | 3-y plan d1 [P] | 3-y plan d16 [P] | Owned d1 [P] | Owned d16 [P] |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|
| meter | Meter gate check + sink emit, in-process | 797,987 | 785,203 | $0.0000155 | $0.0000158 | $0.00000684 | $0.00000695 | $0.00000118 | $0.00000120 |
| iggy | Iggy native TCP produce, 100 B messages | 12,937 | 211,900 | $0.000958 | $0.0000585 | $0.000422 | $0.0000258 | $0.0000726 | $0.00000443 |
| kv | loams-kv embedded (redb, in memory) get/put | 54,218 | 105,969 | $0.000229 | $0.000117 | $0.000101 | $0.0000515 | $0.0000173 | $0.00000886 |
| gateway | loams-gateway HTTP/2 GET /healthz | 41,877 | 67,616 | $0.000296 | $0.000183 | $0.000130 | $0.0000807 | $0.0000224 | $0.0000139 |
| pg | Postgres 17.10 point select (stock compute) | 30,402 | 47,197 | $0.000408 | $0.000263 | $0.000179 | $0.000116 | $0.0000309 | $0.0000199 |
| sql | TiDB (keyspace) point select, MySQL wire | 5,802 | 32,000 | $0.00214 | $0.000387 | $0.000940 | $0.000171 | $0.000162 | $0.0000293 |
| kv-tikv | loams-kv on TiKV get/put | 7,336 | 5,816 | $0.00169 | $0.00213 | $0.000744 | $0.000938 | $0.000128 | $0.000161 |
| git | git-on-S3 WAL push, file store | 372 | 4,455 | $0.0333 | $0.00278 | $0.0147 | $0.00122 | $0.00252 | $0.000211 |
| graph | Grafeo point lookup | 65 | 1,729 | $0.191 | $0.00717 | $0.0840 | $0.00316 | $0.0144 | $0.000543 |
| durable | Resonate promise.create on SQLite | 636 | 684 | $0.0195 | $0.0181 | $0.00857 | $0.00798 | $0.00147 | $0.00137 |
| house | House (chDB worker) point query | 23 | – | $0.533 | – | $0.235 | – | $0.0404 | – |
| **whole mix** | all eleven slots at once | 54,109 | 89,360 | $0.000229 | $0.000139 | $0.000101 | $0.0000611 | $0.0000173 | $0.0000105 |
<!-- /cost:engines -->

How to read it:

- **Pipelining is the biggest lever on cost.** Iggy is 16× cheaper per
  message at depth 16 than at depth 1, TiDB 5.5×, git 12× and graph 27×.
  Postgres, the gateway and the embedded kv gain under 2×, because their
  server core was already busy at depth 1.
- **kv-tikv gets dearer when pipelined.** At depth 16, sql's batched reads
  take the single TiKV core it shares, so its throughput falls (combined
  report).
- **sql looks cheaper than it is.** Its TiKV store runs on the core that is
  booked to kv-tikv, so sql's busy cores leave TiKV out. Giving sql a fair
  share of TiKV's core would raise sql's cost and lower kv-tikv's. We have not
  split it, because the run does not say how much of TiKV's work was sql's.
- **Resonate, graph and the House** are the expensive ones for reasons the
  combined report already names: one SQLite commit per promise, point lookups
  that scan, and one query at a time per chDB worker. Their cost per million
  falls when those are fixed, not when hardware gets cheaper.

### Without the load generator

A deployment does not pay for its clients' cores. For the network engines
whose server ran on a core of its own, this is the same calculation with the
server's cores only [M]. It lowers the cost by 1.5× to 2.7×.

<!-- cost:server-only -->
| Engine (d16) | Server cores busy [M] | ops/s per server core [M] | vs busy-core figure [M] | On-demand $/1M [P] | Owned $/1M [P] |
|---|---:|---:|---:|---:|---:|
| iggy | 0.67 | 324,775 | 1.5× | $0.0000382 | $0.00000289 |
| gateway | 1.00 | 100,662 | 1.5× | $0.000123 | $0.00000932 |
| pg | 1.00 | 93,545 | 2.0× | $0.000133 | $0.0000100 |
| durable | 0.29 | 1,830 | 2.7× | $0.00677 | $0.000513 |
| house | 0.46 | – | – | – | – |
<!-- /cost:server-only -->

## Sensitivity

$ per million operations at depth 16 [P], for a range of $ per core-hour.
Rows are sorted by price. The labels say which inputs are published and which
are assumed.

<!-- cost:sensitivity -->
| $ per core-hour scenario | $/core-h | meter d16 | iggy d16 | gateway d16 | pg d16 | sql d16 | durable d16 | whole mix d16 | Label |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---|
| Owned, 5-year life, 100 W | $0.00244 | $0.000000863 | $0.00000320 | $0.0000100 | $0.0000144 | $0.0000212 | $0.000991 | $0.00000759 | assumption |
| Owned, 28 W (CPU base power) | $0.00263 | $0.000000932 | $0.00000345 | $0.0000108 | $0.0000155 | $0.0000229 | $0.00107 | $0.00000819 | published + assumption |
| Owned, 100 W (PSU rating) | $0.00338 | $0.00000120 | $0.00000443 | $0.0000139 | $0.0000199 | $0.0000293 | $0.00137 | $0.0000105 | published + assumption |
| Owned, RAM + SSD at 2× the assumption | $0.00406 | $0.00000144 | $0.00000532 | $0.0000167 | $0.0000239 | $0.0000352 | $0.00165 | $0.0000126 | assumption |
| AWS EC2 Instance Savings Plan, c7i, 3 years, all upfront, 1 vCPU per core | $0.0171 | $0.00000605 | $0.0000224 | $0.0000702 | $0.000101 | $0.000148 | $0.00694 | $0.0000532 | published × assumption |
| AWS EC2 Instance Savings Plan, c7i, 3 years, no upfront, 1 vCPU per core | $0.0196 | $0.00000695 | $0.0000258 | $0.0000807 | $0.000116 | $0.000171 | $0.00798 | $0.0000611 | published × assumption |
| AWS EC2 Instance Savings Plan, c7i, 3 years, no upfront, 2 vCPU per core | $0.0393 | $0.0000139 | $0.0000515 | $0.000161 | $0.000231 | $0.000341 | $0.0160 | $0.000122 | published × assumption |
| AWS EC2 c8g.large on-demand (Graviton4; 1 vCPU = 1 physical core), 1 vCPU per core | $0.0399 | $0.0000141 | $0.0000523 | $0.000164 | $0.000235 | $0.000346 | $0.0162 | $0.000124 | published × assumption |
| AWS EC2 c7i.large on-demand, 1 vCPU per core | $0.0446 | $0.0000158 | $0.0000585 | $0.000183 | $0.000263 | $0.000387 | $0.0181 | $0.000139 | published × assumption |
| AWS EC2 c8i.large on-demand, 1 vCPU per core | $0.0469 | $0.0000166 | $0.0000614 | $0.000192 | $0.000276 | $0.000407 | $0.0190 | $0.000146 | published × assumption |
| AWS EC2 c7i.large on-demand, 2 vCPU per core | $0.0892 | $0.0000316 | $0.000117 | $0.000367 | $0.000525 | $0.000775 | $0.0362 | $0.000277 | published × assumption |
<!-- /cost:sensitivity -->

The order of engines never changes, because every scenario scales all of
them by the same factor: the ratio between the dearest and cheapest row is
about 37×. The vCPU mapping (1 or 2 per core) moves the cloud figures by 2×;
the choice between on-demand and a 3-year plan by 2.3×; owning the hardware
by a further 6× (at 100 W) to 7.5× (at 28 W).

## Managed-service comparison: Confluent Cloud and Iggy

This is the one comparison where we have both a measured bill and a measured
throughput on each side. It is still not like for like (see the caveats).

<!-- cost:confluent -->
| Confluent Dedicated, per CKU | Throughput per CKU [M] | Wire bytes per record [M] | CKU at $4.00/h, per 1M records [A] | Data in at $0.05/GB [published] | Total per 1M records [P] | Total at $3.00/CKU-h [P] |
|---|---:|---:|---:|---:|---:|---:|
| 1 KB records, cluster-bound | 62.8 MB/s | – | $0.0177 /GB | $0.0500 /GB | $0.0677 /GB | $0.0633 /GB |
| 100 B records, generator-bound (lower bound) | 316,803 rec/s | 103.7 | $0.00351 | $0.00519 | $0.00869 | $0.00782 |
| 10 B payload burst, peak minute (lower bound) | 1,084,919 rec/s | 27.1 | $0.00102 | $0.00135 | $0.00238 | $0.00212 |

| Actual bill (guard estimate, upper bound) [M] | USD [M] | GB ingested [M] | $ per GB ingested [M] |
|---|---:|---:|---:|
| Whole CFB lane (Basic, Standard, Dedicated, Flink generators, idle hours, phase 3) | $1,211 | 3,177 | $0.381 |
| Dedicated test only (per-test recount) | $542 | 1,303 | $0.416 |

| 100 B messages | msgs/s per core [M] | On-demand $/1M, RF 1 [P] | On-demand $/1M, ×3 [P,A] | Owned $/1M, RF 1 [P] | Owned $/1M, ×3 [P,A] | Confluent $0.00869 ÷ on-demand ×3 [P] | Confluent CKU only $0.00351 ÷ on-demand ×3 [P] |
|---|---:|---:|---:|---:|---:|---:|---:|
| Iggy, d16, per busy core (server + load generator) | 211,900 | $0.0000585 | $0.000175 | $0.00000443 | $0.0000133 | 50× | 20× |
| Iggy, d16, per server core only | 324,775 | $0.0000382 | $0.000115 | $0.00000289 | $0.00000867 | 76× | 31× |
| Iggy, d1, per busy core | 12,937 | $0.000958 | $0.00287 | $0.0000726 | $0.000218 | 3.0× | 1.2× |

| Per GB of payload | MB/s per unit [M] | On-demand $/GB, RF 1 [P] | On-demand $/GB, ×3 [P,A] | Owned $/GB, ×3 [P,A] |
|---|---:|---:|---:|---:|
| Iggy per busy core, d16 (100 B payload) | 21.2 | $0.000585 | $0.00175 | $0.000133 |
| Iggy per server core, d16 (100 B payload) | 32.5 | $0.000382 | $0.00115 | $0.0000867 |
| Confluent per CKU, 1 KB records (CKU + data in; replication included; record size differs) | 62.8 | $0.0677 | – | – |
<!-- /cost:confluent -->

What the tables say:

1. **What one CKU does and costs.** At 1 KB records a Dedicated CKU is
   cluster-bound at 62.8 MB/s [M], on AWS, GCP and Azure alike. At an assumed
   $4.00 per CKU-hour [A] that is $0.018 per GB for the CKU [P], and the
   $0.05 per GB data-in charge [published] is the larger part of the total.
   The 100-byte and 10-byte rows are lower bounds on throughput (the Flink
   generators, not the cluster, set the limit), so their dollar figures are
   upper bounds.
2. **What our benchmark actually cost.** $0.38 per GB ingested [M] across
   the whole lane, $0.42 for the Dedicated test alone. Both include Flink
   generators, the hours clusters sat idle, and (for the whole lane) Basic
   and Standard clusters and the phase-3 clusters that ran no load. That is
   what a benchmark costs, not what a steady workload would.
3. **Iggy against Confluent, per 100-byte message.** Iggy's measured
   throughput per core [M], priced at c7i on-demand [published], tripled for
   replication [A], against Confluent's 100-byte figure above. At depth 16
   Iggy is about 50× cheaper counting the load generator's core and 76×
   counting the server only [P]. Against Confluent's CKU charge alone (no
   data-in fee) it is 20× and 31× [P]. At depth 1 (one message per request)
   the gap shrinks to about 3× [P], and to near parity against the CKU charge
   alone. Confluent's Flink producer batched about 160 records per request,
   so depth 16 is the closer match.

### Caveats on this comparison

- **Different hardware.** Iggy ran on one E-core of a laptop over loopback.
  Confluent ran on its own cloud instances behind real networks. The cloud
  price stands in for hardware we did not measure on.
- **Durability.** Iggy ran one server, one shard, with the build's default
  persistence; we did not turn on an fsync per batch, and the run did not
  verify the setting. Confluent Dedicated replicates every record three ways
  across brokers. The "×3" columns are an assumption that a replica costs a
  full server's CPU; they leave out the network between replicas, and
  cross-zone traffic in a cloud is billed.
- **Replication factor.** Iggy's measured numbers are replication factor 1.
  Confluent's are its default (3). Neither run measured the other's setting.
- **What is in the price.** Confluent's price includes operations, upgrades,
  support, storage up to its limits and a multi-tenant control plane. The
  Iggy figures include CPU only: no storage, no network, no people.
- **Record size.** The per-GB table compares Iggy at 100-byte messages with
  Confluent at 1 KB records, because those are the sizes each run measured
  at its limit. Per-byte overhead is higher for small records, which favours
  Confluent in that table.
- **The CKU price is not published.** Confluent sells Dedicated through sales.
  The $4.00 per CKU-hour is the CFB lane's guard rule [A]; third-party list
  estimates are $3.00–3.30, and the first table also shows $3.00.
- **The bill is not final.** $1,211.05 is the guard's estimate, counting
  every started hour at each resource's cap. The Costs API reconciliation is
  due by 2026-11-01 and will replace it; it can only come in lower or equal,
  unless Confluent's Dedicated price is above $4.00.
- **The Kafka wire is not this path.** These are Iggy's native protocol
  numbers. The Iggy Kafka gateway (which Kafka clients would use) has its own
  results in `bench/results/bq1/iggy/ab-*.jsonl`, but no reliable per-core
  CPU figure yet, so it is not priced here.

## Limitations

- **One laptop.** One Intel Core Ultra 5 225H with 15 GB of RAM, shared with
  the desktop. Server CPUs, memory bandwidth and NVMe differ. Some engines ran
  on P-cores, some on E-cores, and the in-process engines (kv, graph, meter,
  git) on the 2.5 GHz low-power cores, so the per-core figures are not on one
  core type.
- **Loopback.** No real network between client and server. A network adds
  latency, interrupts and, in a cloud, data-transfer charges that this report
  does not include.
- **No replication.** Every Loams engine ran as one node, replication factor
  1 (TiKV `max-replicas = 1`). A production deployment pays for its replicas.
- **Quiet window.** Both runs were taken inside the bench lock in a quiet
  window, but the eleven engines shared the machine with each other: load
  average 29–51 and iowait 13–20 %. An engine run alone would show a
  different per-core number.
- **Busy cores include load generators.** For the network engines this makes
  the figures conservative by up to about 2× (the server-only table).
- **Stock Postgres.** The pg slot is stock Postgres 17.10 compute, not the
  Loams Postgres stack with its pageserver and safekeeper.
- **The gateway** measures its HTTP/2 stack on `/healthz`, not a proxied API.
- **CPU only.** Memory, disk, storage capacity, network and operations staff
  are not priced. Engines bound by I/O (Resonate, git, TiKV) are
  under-costed by a CPU-only model.
- **Two runs.** The A and B runs agree within 2 % on totals, but not per
  engine at depth 16. Relative to the mean, kv-tikv differs by 41 % between
  runs (4,991 and 7,572 QPS), git by 33 % and Resonate by 22 % [M]. Their
  dollar figures carry that spread.
- **24/7 utilisation** is assumed for owned hardware. Idle hours raise the
  real cost per operation in proportion.

## Reproducing

```sh
python3 bench/cost/compute.py            # print every table
python3 bench/cost/compute.py --update   # rewrite the tables in this file
python3 bench/cost/compute.py --check    # exit 1 if this file is stale
```

The script uses the Python 3.11+ standard library only. To price a different
machine or cloud, change `bench/cost/prices.toml` (every entry has its
source and label) and run `--update`. New BQ1 combined runs in
`bench/results/bq1/all/` are picked up when their `label` is `final` or
`final-repeat`.

The AWS prices came from the public Price List files:
`https://b0.p.awsstatic.com/pricing/2.0/meteredUnitMaps/ec2/USD/current/ec2-ondemand-without-sec-sel/US%20East%20(N.%20Virginia)/Linux/index.json`
(gzip) for on-demand, and the `AWSComputeSavingsPlan` us-east-1 offer file
from `https://pricing.us-east-1.amazonaws.com/savingsPlan/v1.0/aws/AWSComputeSavingsPlan/current/region_index.json`
for the savings plans (look for `BoxUsage:c7i.large`, operation
`RunInstances`).

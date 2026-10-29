# Confluent benchmark ledger (lane CFB, 2026-10-28)

> **Budget raised 2026-10-28T07:25Z** (owner via coordinator: "you can spend upto 2000 dollrs bro use it"):
> hard cap **$2,000** total including everything already spent; **plan cap $1,900** (`~/.cfb-state/stop_usd`);
> the guard stops everything at an estimate of $1,900. **New hard teardown deadline 2026-10-29T06:00Z.**
> Dedicated CKU-hours are not published; the guard counts **$4.00/CKU-hour** (third-party list estimates
> $3.00-3.30), reconciled with the Costs API later. The guard refreshes data cost from the Metrics API every
> 10 min (since 07:26Z); per-cluster byte totals are kept monotonic because a deleted cluster's metrics answer 403.

Owner, 2026-10-28: a $400 Confluent limit. Confluent has no budget API, so this ledger
and `bench/confluent/guard.sh` enforce it. **Plan cap $380** (margin $20): `cfb.sh`
refuses any create or resize that makes the projected total exceed it. The guard
deletes everything at an **estimate of $380**, on `~/.cfb-state/ABORT`, or at the hard
deadline **2026-10-28T09:00Z**. Environment `env-3kqygo` only; the oracle
(`env-3kqddw`, `lkc-120zwvj`) is refused by `lib/cc.sh`.

## Prices used (list, USD, upper bound where Confluent gives a range)

Source: confluent.io/confluent-cloud/pricing (read 2026-10-26 into
`deploy/tofu/modules/confluent/confluent-tables.json`; re-read 2026-10-28, same figures),
docs.confluent.io Flink billing, billing-dimensions (Cluster Linking).

| Item | Price |
| --- | --- |
| Basic eCKU-hour | first eCKU free, then $0.14 |
| Standard eCKU-hour | $0.75 |
| Enterprise eCKU-hour | $1.75–$2.25 (not used: private networking only; a laptop driver cannot reach it) |
| Dedicated CKU-hour | not published ("contact sales"), so not used |
| Data in / out | $0.05/GB (Standard $0.035–0.05: $0.05 used) |
| Storage | $0.08/GB-month pre-replication (counted x3, CC0 R0.8) |
| Flink | $0.21/CFU-hour, billed per minute |
| Cluster Linking | $0.25/link-hour on each side + $0.25/GB; destination must be Enterprise/Dedicated (not used) |

Billing is per hour on the **maximum eCKU used** in the hour. The ledger counts the
**max eCKU cap** for every started hour (an upper bound); the Costs API reconciles later
(up to 72 h lag). The owner's Cloud key reads the Costs API (HTTP 200, 2026-10-27T21:15Z).

## Plan (projected, worst case)

| # | Resource | Cloud / region | Type | Cap | Hours | $/h at cap | Projected |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | loams-cfb-aws-1 | AWS ap-south-1 (Mumbai) | Basic | 1 eCKU, 50 for 2 h push | 10 | 0 / 6.86 | $13.72 |
| 2 | loams-cfb-gcp-1 | GCP asia-south1 (Mumbai) | Basic | same | 10 | 0 / 6.86 | $13.72 |
| 3 | loams-cfb-azure-1 | Azure centralindia (Pune) | Basic | same | 10 | 0 / 6.86 | $13.72 |
| 4 | loams-cfb-aws-us-1 | AWS us-east-1 | Basic | 1 eCKU | 10 | 0 | $0 |
| 5 | loams-cfb-aws-2 | AWS ap-south-1 | Standard | 4 eCKU 6 h + 10 eCKU 2 h | 8 | 3.00 / 7.50 | $33.00 |
| 6 | loams-cfb-gcp-2 | GCP asia-south1 | Standard | same | 8 | 3.00 / 7.50 | $33.00 |
| 7 | loams-cfb-azure-2 | Azure centralindia | Standard | same | 8 | 3.00 / 7.50 | $33.00 |
| 8 | Flink pools x3 (in-region load generators) | the three Mumbai/Pune regions | Flink | 20 CFU | 3 | 4.20 each | $37.80 |
| 9 | In-region push data in | clusters 1–3, 5–7 | data | ≤ 250 MB/s for 20 min each | — | — | $90.00 |
| 10 | WAN driver data in + out (laptop) | all | data | < 20 GB | — | — | $2.00 |
| | **Total projected** | | | | | | **$269.96** |

Headroom to the $380 cap: about $110. That is kept for a longer soak or a second
push if the first results warrant it; every addition is checked by `cfb.sh` first.

## Resources (append as created; running total = guard estimate at that time)

| UTC | Resource | Id | Cloud / region | Type | Cap | $/h | Deadline | Running est. |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 2026-10-27T21:19Z | loams-cfb-aws-1 | lkc-81dm7dq | AWS ap-south-1 | Basic | 1 eCKU | $0.00 | 07:19Z | $0.00 |
| 2026-10-27T21:19Z | loams-cfb-gcp-1 | lkc-k8wg2w2 | GCP asia-south1 | Basic | 1 eCKU | $0.00 | 07:19Z | $0.00 |
| 2026-10-27T21:19Z | loams-cfb-azure-1 | lkc-zmk3zk7 | Azure centralindia | Basic | 1 eCKU | $0.00 | 07:19Z | $0.00 |
| 2026-10-27T21:19Z | loams-cfb-aws-us-1 | lkc-o36276o | AWS us-east-1 | Basic | 1 eCKU | $0.00 | 07:19Z | $0.00 |
| 2026-10-28T02:03Z | loams-cfb-aws-2 | lkc-k8wgvy6 | AWS ap-south-1 | Standard | 4 eCKU | $3.00 | 09:00Z (hard) | $63.00 projected for the 3 Standards |
| 2026-10-28T02:03Z | loams-cfb-gcp-2 | lkc-57ymjvz | GCP asia-south1 | Standard | 4 eCKU | $3.00 | 09:00Z (hard) | |
| 2026-10-28T02:03Z | loams-cfb-azure-2 | lkc-dozgnyz | Azure centralindia | Standard | 4 eCKU | $3.00 | 09:00Z (hard) | |
| 2026-10-28T02:04Z | loams-cfb-flink-aws (pool) | lfcp-6kyorx6 | AWS ap-south-1 | Flink | 20 CFU | $4.20 | 06:04Z | $79.80 projected total |
| 2026-10-28T02:15Z | loams-cfb-flink-gcp (pool) | lfcp-22y3z7y | GCP asia-south1 | Flink | 20 CFU | $4.20 | 06:15Z | |
| 2026-10-28T02:15Z | loams-cfb-flink-azure (pool) | lfcp-81dm0x7 | Azure centralindia | Flink | 20 CFU | $4.20 | 06:15Z | |
| 2026-10-28T02:15Z | loams-cfb-bench (SA) + rb-qqbqLA | sa-8mq9vkq | org / env-3kqygo | service account | – | $0 | teardown | owns keys beyond the 10-per-user quota |
| 2026-10-28T02:18-02:45Z | Basic pushes: aws-1 up to 25 eCKU, gcp-1 up to 50, azure-1 up to 50 then 1 | | | Basic | | ≤ $6.86 | | stuck at 25/50 until 07:20Z (race + pause) |
| 2026-10-28T07:27Z | loams-cfb-aws-d1 | lkc-12ywwyj | AWS ap-south-1 | Dedicated | 1 CKU | $4.00 (assumed) | 15:29Z | $359 projected |
| 2026-10-28T07:27Z | loams-cfb-aws-d4 | lkc-o36xx6x | AWS ap-south-1 | Dedicated | 4 CKU | $16.00 (assumed) | 15:29Z | $551 projected |
| 2026-10-28T07:27Z | loams-cfb-aws-d2 | lkc-7ydqqdp | AWS ap-south-1 | Dedicated | 2 CKU | deleted while provisioning (3-provisioning quota) | – | – |
| 2026-10-28T07:29Z | loams-cfb-gcp-d2 | lkc-k8wkkwv | GCP asia-south1 | Dedicated | 2 CKU | $8.00 (assumed) | 15:29Z | $559 projected |
| 2026-10-28T07:24Z | loams-cfb-flink-azure2 (pool) | lfcp-9kdzzg5 | Azure centralindia | Flink | 20 CFU | $4.20 | 08:24Z | $327 projected |

| 2026-10-28T07:40Z | loams-cfb-azure-d2 | lkc-xqk77nz | Azure centralindia | Dedicated | 2 CKU | $8.00 (assumed) | 15:40Z | |
| 2026-10-28T12:13Z | loams-cfb-flink-gcp3 / -azure3 (pools) | lfcp-dozvyg1 / lfcp-38dwgn0 | GCP asia-south1 / Azure centralindia | Flink | 30 CFU each | $6.30 each | 15:13Z | |
| 2026-10-28T12:14Z | loams-cfb-flink-aws4 / -aws1 (pools) | lfcp-7ydq9p1 / lfcp-q26j787 | AWS ap-south-1 | Flink | 50 / 20 CFU | $10.50 / $4.20 | 15:14Z | $719 projected |
| 2026-10-28T12:27Z | links cfb-gcp-d2-to-aws-d4, cfb-azure-d2-to-aws-d4 | – | GCP->AWS, Azure->AWS | Cluster Linking | 2 links | $1.00 (4 sides x $0.25) | with aws-d4 | +$3 booked for 3 h |

Events: 09:00Z guard deleted SA loams-cfb-bench (its row carried the old 09:00Z hard deadline; its keys went with it); 09:04Z guard deleted the three Standard clusters at deadline; 12:12Z owner keys minted for the 4 Dedicated clusters, which idled ~07:40-12:14Z during a session pause. 06:04/06:15Z guard deleted the three Flink pools at deadline; 07:20Z guard deleted the four Basic clusters at deadline. 07:22Z Standard x3 lowered to 1 eCKU.

## Final (2026-10-28T17:25Z): everything deleted

- 17:12Z all four Dedicated clusters deleted by hand (`cfb.sh delete`). The guard process had died at ~15:21Z, so their 15:29/15:40Z deadlines never fired.
- Verified at 17:25Z: env-3kqygo has 0 clusters, 0 Flink pools, no `loams-cfb*` API keys; SA sa-8mq9vkq GET 404; rb-qqbqLA GET 403 (gone). Oracle lkc-120zwvj untouched (read only: PROVISIONED).
- **Spend (upper bound): guard $886.54 (binding); per-test recount $822.64** (Basic $200.24, Standard $75.03, Dedicated $542.36 incl. 2.85 TB data, Cluster Linking $5). Dedicated priced at an assumed $4.00/CKU-h. The Costs API reconciliation is due by 2026-10-31.
- Guard hardening after this run: a systemd --user timer (`guard-timer.sh`), an idle rule (paid cluster with < 1 MB ingress in 30 min is deleted), data totals kept for deleted clusters, a fix for the empty-live-file estimate, and a lock on live.tsv.

## Reconciliation (hourly: guard estimate vs Metrics API bytes vs Costs API)

| UTC | Guard estimate | Data (Metrics API) | Costs API (lags ≤ 72 h) | Note |
| --- | --- | --- | --- | --- |
| 2026-10-27T21:15Z | $0.00 | $0.00 | -$0.0004 for 2026-10-20..27 (6 lines) | baseline before CFB |
| 2026-10-28T17:20Z | $886.54 | $142.36 (2,847 GB in) | pending | final; per-test recount $822.64 |
| 2026-10-28T12:12Z | $529.17 | $77.19 (1,544 GB in) | pending | Dedicated idle during the pause counted at $4/CKU-h |
| 2026-10-28T07:22Z | $318.64 | $69.06 (1,381 GB in, Basic pushes) | pending (72 h lag) | eCKU counted at cap for every started hour since creation: an over-count |

## 2026-10-28 budget raise
Owner, 2026-10-28: "you can spend upto 2000 dollrs bro use it". New hard cap $2,000 total (including spend so far), plan <= $1,900. Guard estimate at time of raise: $320.89. All other Confluent rules unchanged.

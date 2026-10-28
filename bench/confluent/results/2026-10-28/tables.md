### WAN round trips from the driver laptop (p50 / p99, ms)

| Cluster | Cloud / region | Type | TCP connect | TLS handshake | Kafka metadata request |
| --- | --- | --- | --- | --- | --- |
| aws-1 | aws ap-south-1 | basic | 146.9 / 548.4 | 209.8 / 3,472.6 | 271.4 / 1,236.4 |
| aws-2 | aws ap-south-1 | standard | 308.9 / 598.8 | 198.5 / 275.9 | 189.3 / 1,234.1 |
| aws-us-1 | aws us-east-1 | basic | 481.0 / 1,342.2 | 411.5 / 956.6 | 511.9 / 3,099.8 |
| azure-1 | azure centralindia | basic | 260.6 / 615.1 | 366.6 / 5,225.7 | 274.7 / 2,236.1 |
| azure-2 | azure centralindia | standard | 175.0 / 1,289.4 | 171.6 / 1,892.0 | 176.6 / 793.8 |
| gcp-1 | gcp asia-south1 | basic | 189.7 / 327.2 | 182.7 / 408.7 | 268.9 / 1,038.0 |
| gcp-2 | gcp asia-south1 | standard | 271.0 / 9,205.2 | 1,252.3 / 9,177.2 | 206.0 / 1,130.1 |

### Produce and end-to-end latency at 10 msg/s (ms; p50 / p99 / p99.9)

| Target | Size | acks | Produce p50 / p99 / p99.9 | End to end p50 / p99 / p99.9 | Sent | Lost after ack | Errors |
| --- | --- | --- | --- | --- | --- | --- | --- |
| aws-1 | 100 B | 1 | 243.1 / 2,928.1 / 3,788.8 | 405.3 / 4,051.4 / 4,544.2 | 600 | 0 | 0 |
| aws-1 | 100 B | all | 6,503.6 / 20,880.0 / 22,979.9 | 7,450.8 / 22,684.5 / 23,584.5 | 600 | 0 | 0 |
| aws-1 | 1024 B | 1 | 253.9 / 2,101.9 / 2,701.9 | 432.0 / 2,402.0 / 3,002.0 | 600 | 0 | 0 |
| aws-1 | 1024 B | all | 274.3 / 4,508.9 / 5,108.9 | 413.5 / 5,002.6 / 5,602.6 | 600 | 0 | 0 |
| aws-1 | 10240 B | 1 | 335.6 / 12,310.0 / 13,209.9 | 583.7 / 12,512.1 / 13,412.1 | 600 | 0 | 0 |
| aws-1 | 10240 B | all | 265.7 / 6,412.7 / 7,212.7 | 444.1 / 7,102.9 / 7,826.0 | 600 | 0 | 0 |
| aws-2 | 100 B | 1 | 184.0 / 1,196.0 / 1,723.5 | 361.1 / 1,482.6 / 1,902.6 | 600 | 0 | 0 |
| aws-2 | 100 B | all | 193.9 / 2,728.3 / 3,331.5 | 429.9 / 3,101.8 / 3,701.8 | 600 | 0 | 0 |
| aws-2 | 1024 B | 1 | 390.4 / 21,678.2 / 22,232.5 | 669.6 / 22,856.4 / 23,456.4 | 600 | 0 | 0 |
| aws-2 | 1024 B | all | 285.1 / 2,689.8 / 4,331.2 | 508.3 / 2,902.0 / 4,406.4 | 600 | 0 | 0 |
| aws-2 | 10240 B | 1 | 532.0 / 4,957.5 / 6,637.9 | 862.4 / 5,653.4 / 7,559.0 | 600 | 0 | 0 |
| aws-2 | 10240 B | all | 7,058.9 / 28,000.1 / 29,000.1 | 7,904.8 / 29,159.9 / 30,159.9 | 600 | 0 | 118 |
| azure-2 | – | – | failed: KafkaException(KafkaError{code=_TIMED_OUT,val=-185,str="Failed while waiting for | | | | |
| azure-2 | – | – | failed: KafkaException(KafkaError{code=_TIMED_OUT,val=-185,str="Failed while waiting for | | | | |
| azure-2 | 100 B | 1 | – / – / – | – / – / – | 600 | 0 | 600 |
| azure-2 | 100 B | all | 3,805.1 / 34,299.5 / 36,699.5 | 57,685.5 / 95,584.4 / 96,885.5 | 600 | 0 | 0 |
| azure-2 | 1024 B | all | 49,791.0 / 80,086.7 / 80,891.0 | 52,346.9 / 81,751.5 / 83,951.5 | 600 | 0 | 0 |
| azure-2 | 10240 B | all | 345.1 / 11,886.8 / 13,697.9 | 515.0 / 13,110.3 / 14,013.5 | 600 | 0 | 0 |
| gcp-1 | 100 B | 1 | 265.1 / 2,469.9 / 3,076.4 | 413.7 / 3,657.4 / 4,057.4 | 600 | 0 | 0 |
| gcp-1 | 100 B | all | 254.9 / 2,234.3 / 2,834.2 | 391.9 / 2,379.4 / 2,979.4 | 600 | 0 | 0 |
| gcp-2 | – | – | failed: KafkaException(KafkaError{code=_TIMED_OUT,val=-185,str="Failed while waiting for | | | | |
| gcp-2 | – | – | failed: KafkaException(KafkaError{code=_TIMED_OUT,val=-185,str="Failed while waiting for | | | | |
| gcp-2 | 100 B | all | 595.6 / 22,572.9 / 23,572.8 | 2,445.0 / 30,025.8 / 31,625.8 | 600 | 0 | 0 |
| gcp-2 | 1024 B | 1 | 50,623.2 / 119,200.2 / 120,500.2 | 90,609.5 / 140,737.2 / 142,037.2 | 600 | 0 | 0 |
| gcp-2 | 1024 B | all | 12,013.9 / 78,240.8 / 80,440.8 | 15,044.0 / 79,033.4 / 81,233.4 | 600 | 0 | 0 |
| gcp-2 | 10240 B | all | 5,271.9 / 28,399.8 / 28,399.8 | 51,631.5 / 56,536.3 / 56,536.3 | 600 | 50 | 517 |

### Transactions (60 transactions x 10 records of 512 B, every 5th aborted)

| Target | init_transactions | Commit p50 / p99 | Abort p50 | Txn/s | read_committed: missing / saw aborted / duplicates | read_uncommitted saw aborted | EOS |
| --- | --- | --- | --- | --- | --- | --- | --- |
| aws-2 | 2,714 ms | 511.0 / 1,439.1 ms | 199.7 ms | 1.89 | 0 / 0 / 0 | 120 of 120 | pass |
| azure-2 | 4,025 ms | 609.8 / 1,641.6 ms | 203.6 ms | 1.7 | 0 / 0 / 0 | 120 of 120 | pass |
| gcp-2 | 9,509 ms | 469.6 / 1,637.8 ms | 125.3 ms | 1.99 | 0 / 0 / 0 | 120 of 120 | pass |
| Kafka 4.3.1 (local) | 60 ms | – / – ms | – ms | 0.0 | 0 / 0 / 0 | 0 of 0 | FAIL |
| Iggy gateway (local) | 103 ms | 35.3 / 1,284.4 ms | 6.1 ms | 13.56 | 0 / 120 / 0 | 120 of 120 | FAIL |

### Producer throughput (acks=all, no compression)

| Target | Where | Record size | MB/s | records/s | Produce p50 / p99 (ms) |
| --- | --- | --- | --- | --- | --- |
| aws-2 | laptop over WAN | 16384 B | 0.6428 | 39.2 | 32,663.4 / 78,270.9 |
| Kafka 4.3.1 (local) | laptop, loopback | 1024 B | 76.8919 | 75,089.7 | 1,095.2 / 3,454.6 |
| Kafka 4.3.1 (local) | laptop, loopback | 16384 B | 59.2385 | 3,615.6 | 12,391.5 / 29,083.9 |
| Iggy gateway (local) | laptop, loopback | 1024 B | 66.767 | 65,202.1 | 39.5 / 10,454.0 |
| Iggy gateway (local) | laptop, loopback | 16384 B | 77.015 | 4,700.6 | 10,212.7 / 28,983.4 |
| aws-2 | laptop over WAN, in parallel on all clusters | 16384 B | 0.5241 | 32.0 | 62,086.6 / 96,011.5 |
| azure-2 | laptop over WAN, in parallel on all clusters | 16384 B | 0.7039 | 43.0 | 27,203.5 / 71,475.4 |
| gcp-2 | laptop over WAN, in parallel on all clusters | 16384 B | 0.645 | 39.4 | 25,689.1 / 77,992.4 |

### In-region push (Flink generator in the same region; Metrics API, per minute)

| Cluster | Type | max eCKU step | Nominal ingress (MB/s) | Measured, steady minutes (MB/s, median) | Peak minute (MB/s) |
| --- | --- | --- | --- | --- | --- |
| aws-1 | basic | 1 | 5 | 7.4 | 12.9 |
| aws-1 | basic | 4 | 20 | 15.0 | 25.7 |
| aws-1 | basic | 10 | 50 | 54.6 | 66.8 |
| aws-1 | basic | 25 | 125 | 56.1 | 70.7 |
| azure-1 | basic | 1 | 5 | 6.2 | 11.9 |
| azure-1 | basic | 4 | 20 | 23.2 | 25.7 |
| azure-1 | basic | 10 | 50 | 54.1 | 61.9 |
| azure-1 | basic | 25 | 125 | 53.9 | 60.1 |
| azure-1 | basic | 50 | 250 | 54.6 | 56.6 |
| azure-1 | basic | 1 | 5 | 5.2 | 40.9 |
| gcp-1 | basic | 1 | 5 | 10.4 | 10.4 |
| gcp-1 | basic | 4 | 20 | 19.4 | 34.6 |
| gcp-1 | basic | 10 | 50 | 39.1 | 44.5 |
| gcp-1 | basic | 25 | 125 | 35.5 | 45.3 |


SET 'execution.runtime-mode' = 'streaming';

SET 'execution.checkpointing.interval' = '10 s';

SET 'table.local-time-zone' = 'UTC';

SET 'pipeline.name' = 'clicks-per-minute';

CREATE CATALOG IF NOT EXISTS `loams` WITH (
  'type' = 'iceberg',
  'catalog-type' = 'rest',
  'uri' = 'http://lakekeeper:8181/catalog',
  'warehouse' = 'prod-eu',
  'base-namespace' = 'ns_7f3k',
  'oauth2-server-uri' = 'https://auth.loams.dev/oauth/v2/token',
  'credential' = 'flink-eng_demo:s3cr''et',
  'scope' = 'lakekeeper',
  'io-impl' = 'org.apache.iceberg.aws.s3.S3FileIO',
  's3.path-style-access' = 'true',
  'client.region' = 'us-east-1',
  'header.X-Iceberg-Access-Delegation' = 'vended-credentials'
);

CREATE TEMPORARY TABLE IF NOT EXISTS `events` (
  `id` STRING,
  `proc` AS PROCTIME()
) WITH (
  'connector' = 'kafka',
  'topic' = 'ns_7f3k.events',
  'properties.bootstrap.servers' = 'kafka.loams.internal:9094',
  'properties.security.protocol' = 'SASL_SSL',
  'properties.sasl.mechanism' = 'PLAIN',
  'properties.sasl.jaas.config' = 'org.apache.flink.kafka.shaded.org.apache.kafka.common.security.plain.PlainLoginModule required username="key_01" password="lk_\"quoted\"\\x";',
  'properties.group.id' = 'loams-flink-events',
  'scan.startup.mode' = 'timestamp',
  'scan.startup.timestamp-millis' = '1793059200000',
  'format' = 'json',
  'json.timestamp-format.standard' = 'ISO-8601',
  'json.ignore-parse-errors' = 'true',
  'json.fail-on-missing-field' = 'false',
  'sink.delivery-guarantee' = 'at-least-once'
);

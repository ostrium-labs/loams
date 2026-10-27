SET 'execution.runtime-mode' = 'streaming';

SET 'execution.checkpointing.interval' = '30 s';

SET 'table.local-time-zone' = 'UTC';

SET 'execution.checkpointing.savepoint-dir' = 's3://loams/engines/acme/eng_demo/savepoints';

CREATE CATALOG IF NOT EXISTS `loams` WITH (
  'type' = 'iceberg',
  'catalog-type' = 'rest',
  'uri' = 'http://lakekeeper:8181/catalog',
  'warehouse' = 'loams',
  'base-namespace' = 'ns_acme',
  'io-impl' = 'org.apache.iceberg.aws.s3.S3FileIO',
  's3.endpoint' = 'http://rustfs:9000',
  's3.path-style-access' = 'true',
  'client.region' = 'us-east-1',
  's3.access-key-id' = 'loams-dev',
  's3.secret-access-key' = '${secret:s3-secret}'
);

CREATE CATALOG IF NOT EXISTS `loams_pg` WITH (
  'type' = 'jdbc',
  'base-url' = 'jdbc:postgresql://compute1:5432/',
  'default-database' = 'shop',
  'username' = 'flink',
  'password' = '${secret:pg-password}'
);

CREATE TEMPORARY TABLE IF NOT EXISTS `clicks` (
  `user_id` STRING,
  `url` STRING,
  `ts` TIMESTAMP_LTZ(3),
  `offset` BIGINT METADATA VIRTUAL,
  WATERMARK FOR `ts` AS `ts` - INTERVAL '5' SECOND
) WITH (
  'connector' = 'kafka',
  'topic' = 'clicks',
  'properties.bootstrap.servers' = 'kafka-gateway:9093',
  'properties.security.protocol' = 'SASL_PLAINTEXT',
  'properties.sasl.mechanism' = 'PLAIN',
  'properties.sasl.jaas.config' = 'org.apache.flink.kafka.shaded.org.apache.kafka.common.security.plain.PlainLoginModule required username="key_01" password="${secret:loams-api-key}";',
  'properties.group.id' = 'loams-flink-clicks',
  'scan.startup.mode' = 'earliest-offset',
  'format' = 'json',
  'json.timestamp-format.standard' = 'ISO-8601',
  'sink.delivery-guarantee' = 'at-least-once'
);

CREATE TEMPORARY TABLE IF NOT EXISTS `user_profiles` (
  `user_id` STRING NOT NULL,
  `plan` STRING,
  PRIMARY KEY (`user_id`) NOT ENFORCED
) WITH (
  'connector' = 'upsert-kafka',
  'topic' = 'user-profiles',
  'properties.bootstrap.servers' = 'kafka-gateway:9093',
  'properties.group.id' = 'loams-flink-user_profiles',
  'key.format' = 'json',
  'key.json.timestamp-format.standard' = 'ISO-8601',
  'value.format' = 'json',
  'value.json.timestamp-format.standard' = 'ISO-8601',
  'sink.delivery-guarantee' = 'at-least-once'
);

CREATE TEMPORARY TABLE IF NOT EXISTS `orders` (
  `id` BIGINT NOT NULL,
  `amount` DECIMAL(10, 2),
  `op_ts` TIMESTAMP_LTZ(3) METADATA VIRTUAL,
  PRIMARY KEY (`id`) NOT ENFORCED
) WITH (
  'connector' = 'postgres-cdc',
  'hostname' = 'compute1',
  'port' = '5432',
  'username' = 'flink_cdc',
  'password' = '${secret:pg-password}',
  'database-name' = 'shop',
  'schema-name' = 'public',
  'table-name' = 'orders',
  'slot.name' = 'loams_fk_01jbx3k9q2w7r5t8y6u4i0p3a_1',
  'decoding.plugin.name' = 'pgoutput',
  'scan.incremental.snapshot.enabled' = 'true',
  'scan.startup.mode' = 'initial',
  'debezium.publication.name' = 'loams_fk_01jbx3k9q2w7r5t8y6u4i0p3a_pub',
  'debezium.publication.autocreate.mode' = 'disabled',
  'debezium.database.sslmode' = 'require'
);

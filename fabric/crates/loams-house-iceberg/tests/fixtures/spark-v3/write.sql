-- Iceberg v3 tables written by unmodified Spark 4.0.4 + Iceberg 1.12.0 for
-- `spark_v3_tables_read_as_spark` (plan HE1 Task 26), from the HE0 Task 11
-- spike's `spark/01-write-v3.sql` (variant and default-value tables left out:
-- the engine refuses Variant, R26.7). Run by scripts/house/spark-v3-fixtures.sh.
CREATE NAMESPACE IF NOT EXISTS spark.v3;

-- 1. Deletion vectors: merge-on-read deletes and an update on an unpartitioned table.
CREATE TABLE spark.v3.dv (id BIGINT, v STRING) USING iceberg
  TBLPROPERTIES ('format-version'='3', 'write.delete.mode'='merge-on-read',
                 'write.update.mode'='merge-on-read', 'write.merge.mode'='merge-on-read');
INSERT INTO spark.v3.dv SELECT id, concat('v', id) FROM range(0, 1000);
DELETE FROM spark.v3.dv WHERE id % 10 = 0;
DELETE FROM spark.v3.dv WHERE id % 10 = 1;
UPDATE spark.v3.dv SET v = 'u' WHERE id % 10 = 2;

-- 2. Deletion vectors on a partitioned table, several data files per partition.
CREATE TABLE spark.v3.dv_part (id BIGINT, p INT, v STRING) USING iceberg PARTITIONED BY (p)
  TBLPROPERTIES ('format-version'='3', 'write.delete.mode'='merge-on-read',
                 'write.update.mode'='merge-on-read', 'write.merge.mode'='merge-on-read');
INSERT INTO spark.v3.dv_part SELECT id, CAST(id % 4 AS INT), concat('v', id) FROM range(0, 2000);
INSERT INTO spark.v3.dv_part SELECT id, CAST(id % 4 AS INT), concat('w', id) FROM range(2000, 3000);
DELETE FROM spark.v3.dv_part WHERE id % 7 = 0;
MERGE INTO spark.v3.dv_part t USING (SELECT id, CAST(id % 4 AS INT) AS p FROM range(0, 3000, 5)) s
  ON t.id = s.id WHEN MATCHED THEN UPDATE SET v = 'm';

-- 3. Row lineage: two appends, then an update; lineage must survive the update.
CREATE TABLE spark.v3.lineage (id BIGINT, v STRING) USING iceberg
  TBLPROPERTIES ('format-version'='3', 'write.update.mode'='merge-on-read',
                 'write.delete.mode'='merge-on-read');
INSERT INTO spark.v3.lineage SELECT id, 'a' FROM range(0, 50);
INSERT INTO spark.v3.lineage SELECT id, 'b' FROM range(50, 100);
UPDATE spark.v3.lineage SET v = 'c' WHERE id % 10 = 3;

-- 4. Control: the same deletes on a v2 table (position delete files).
CREATE TABLE spark.v3.posdel_v2 (id BIGINT, v STRING) USING iceberg
  TBLPROPERTIES ('format-version'='2', 'write.delete.mode'='merge-on-read',
                 'write.update.mode'='merge-on-read');
INSERT INTO spark.v3.posdel_v2 SELECT id, concat('v', id) FROM range(0, 1000);
DELETE FROM spark.v3.posdel_v2 WHERE id % 10 = 0;
UPDATE spark.v3.posdel_v2 SET v = 'u' WHERE id % 10 = 2;

-- 5. Upgrade in place: a v2 table with a position delete, upgraded to v3,
-- then deleted from again (the first deletion vector merges the v2 deletes).
CREATE TABLE spark.v3.upgraded (id BIGINT, v STRING) USING iceberg
  TBLPROPERTIES ('format-version'='2', 'write.delete.mode'='merge-on-read');
INSERT INTO spark.v3.upgraded SELECT id, concat('v', id) FROM range(0, 1000);
DELETE FROM spark.v3.upgraded WHERE id % 10 = 0;
ALTER TABLE spark.v3.upgraded SET TBLPROPERTIES ('format-version'='3');
INSERT INTO spark.v3.upgraded SELECT id, concat('v', id) FROM range(1000, 1100);
DELETE FROM spark.v3.upgraded WHERE id % 10 = 1;

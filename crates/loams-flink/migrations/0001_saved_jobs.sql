-- Saved Flink SQL jobs (design 81 D1988, ENG81 R21): one row per job, the
-- whole record as JSON plus the columns the store filters and compares on.
-- Secret references only, never values (D1985).
CREATE TABLE flink_saved_jobs (
    namespace  text   NOT NULL,
    id         text   NOT NULL,
    instance   text   NOT NULL,
    state      text   NOT NULL,
    version    bigint NOT NULL,
    created_at text   NOT NULL,
    body       jsonb  NOT NULL,
    PRIMARY KEY (namespace, id)
);

CREATE INDEX flink_saved_jobs_by_instance
    ON flink_saved_jobs (namespace, instance, created_at, id);

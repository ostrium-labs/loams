-- LC0 Task 3: the control-plane records, on the free Starter instance `lc0-worker`.
-- Applied with `ti db execute-sql-statement --admin` one statement at a time (setup.sh).
CREATE DATABASE IF NOT EXISTS lc0;
CREATE TABLE IF NOT EXISTS lc0.devices (user_id VARCHAR(64) NOT NULL, device_id VARCHAR(64) NOT NULL, name VARCHAR(128) NOT NULL, state VARCHAR(16) NOT NULL, last_seen_ms BIGINT NOT NULL, updated_ms BIGINT NOT NULL, PRIMARY KEY (user_id, device_id));
CREATE TABLE IF NOT EXISTS lc0.leases (user_id VARCHAR(64) NOT NULL, run_id VARCHAR(64) NOT NULL, holder VARCHAR(64) NOT NULL, epoch BIGINT NOT NULL, expires_ms BIGINT NOT NULL, updated_ms BIGINT NOT NULL, PRIMARY KEY (user_id, run_id));
CREATE TABLE IF NOT EXISTS lc0.lease_events (user_id VARCHAR(64) NOT NULL, run_id VARCHAR(64) NOT NULL, epoch BIGINT NOT NULL, holder VARCHAR(64) NOT NULL, at_ms BIGINT NOT NULL, PRIMARY KEY (user_id, run_id, epoch));
CREATE TABLE IF NOT EXISTS lc0.run_intents (idem_key VARCHAR(64) NOT NULL PRIMARY KEY, user_id VARCHAR(64) NOT NULL, run_id VARCHAR(64) NOT NULL, target_device VARCHAR(64) NOT NULL, payload_hash CHAR(64) NOT NULL, created_ms BIGINT NOT NULL);
CREATE TABLE IF NOT EXISTS lc0.bench (id BIGINT AUTO_RANDOM PRIMARY KEY, k VARCHAR(64) NOT NULL, v VARCHAR(255) NOT NULL, at_ms BIGINT NOT NULL, UNIQUE KEY (k));

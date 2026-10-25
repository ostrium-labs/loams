-- Loams Cloud control DB, migration 0001: the waitlist, invites and accounts
-- (design 78 §25.3, D1739, D1740; plan LC1, LC2a Task 1).
--
-- Target: the control plane's own TiDB Cloud Starter instance (D1717). The schema
-- stays inside Starter's features: no TTL attribute, no partitioning, no foreign
-- keys, no CHECK constraints. Retention is enforced by the Worker's jobs (§25.6),
-- and every state column lists its values in a comment instead of a CHECK.
--
-- Times are BIGINT milliseconds since the Unix epoch (UTC), as in LC0's spike.
-- Ids are prefixed random strings minted by the Worker (wl_, inv_, acct_, evt_),
-- so every insert can be retried with the same id: LC0 Task 3 found that an
-- aborted write still commits, so no write may rely on "the call failed" meaning
-- "nothing was written" (lc0-report §4).
--
-- Addresses: never in plaintext. `*_hmac` is HMAC-SHA-256 (hex) of the
-- normalised address under a Worker secret; `*_enc` is AES-256-GCM (base64 of
-- iv || ciphertext) under a second secret. A dump without the keys holds no
-- readable address. `no_plaintext_email_column_outside_enc` holds that line.
--
-- No statement contains a `;` inside a literal, so the migrator (src/schema.ts)
-- splits on `;` after dropping `--` comments and sends the statements one at a
-- time: the HTTP driver runs one statement per request. Every statement is
-- idempotent (IF NOT EXISTS, INSERT IGNORE), so a retried migration is safe.

-- The migrations this database has applied.
CREATE TABLE IF NOT EXISTS schema_migrations (
  version     INT          NOT NULL PRIMARY KEY,
  name        VARCHAR(64)  NOT NULL,
  applied_ms  BIGINT       NOT NULL
);

-- One row per address that joined the waitlist (§25.3).
CREATE TABLE IF NOT EXISTS waitlist (
  id                    VARCHAR(40)  NOT NULL PRIMARY KEY,         -- wl_ + 128-bit random
  email_hmac            CHAR(64)     NOT NULL,
  email_enc             VARCHAR(512) NOT NULL,
  status                VARCHAR(16)  NOT NULL,                     -- pending | confirmed | invited
  build                 VARCHAR(500) NULL,                         -- "what will you build", optional
  source                VARCHAR(16)  NULL,                         -- search | github | social | friend | event | other
  source_other          VARCHAR(100) NULL,
  created_ms            BIGINT       NOT NULL,
  confirmed_ms          BIGINT       NULL,
  updated_ms            BIGINT       NOT NULL,
  ip_prefix_hmac        CHAR(64)     NULL,                         -- /24 or /48 prefix HMAC, cleared after 7 days
  ip_prefix_expires_ms  BIGINT       NULL,
  already_listed_ms     BIGINT       NULL,                         -- the last "already on the list" email (at most one a day)
  reminder_sent_ms      BIGINT       NULL,                         -- the 12-month "still interested?" email
  UNIQUE KEY uk_waitlist_email_hmac (email_hmac),
  KEY k_waitlist_status_created (status, created_ms),
  KEY k_waitlist_source_created (source, created_ms)
);

-- Our own confirm and removal links (D1740). Only SHA-256 of the token is kept.
CREATE TABLE IF NOT EXISTS waitlist_tokens (
  token_hash   CHAR(64)    NOT NULL PRIMARY KEY,
  waitlist_id  VARCHAR(40) NOT NULL,
  kind         VARCHAR(8)  NOT NULL,                              -- confirm | remove
  created_ms   BIGINT      NOT NULL,
  expires_ms   BIGINT      NOT NULL,                              -- confirm 48 h, remove 90 days
  used_ms      BIGINT      NULL,
  KEY k_tokens_waitlist (waitlist_id, kind),
  KEY k_tokens_expires (expires_ms)
);

-- Addresses that must never be mailed or re-added: only the HMAC survives.
CREATE TABLE IF NOT EXISTS waitlist_suppressed (
  email_hmac  CHAR(64)    NOT NULL PRIMARY KEY,
  reason      VARCHAR(16) NOT NULL,                               -- removed | deleted | complaint | hard_bounce
  created_ms  BIGINT      NOT NULL,
  KEY k_suppressed_created (created_ms)
);

-- Invitations. WorkOS holds the invitation token and URL; we keep only its id (D1739).
CREATE TABLE IF NOT EXISTS invites (
  id                    VARCHAR(40)  NOT NULL PRIMARY KEY,         -- inv_ + 128-bit random
  waitlist_id           VARCHAR(40)  NOT NULL,
  email_hmac            CHAR(64)     NOT NULL,
  workos_invitation_id  VARCHAR(64)  NULL,                         -- set once WorkOS has created it
  state                 VARCHAR(16)  NOT NULL,                     -- queued | pending | accepted | expired | revoked | failed
  open_key              VARCHAR(40)  NULL,                         -- = waitlist_id while queued, pending or accepted; NULL once closed
  expires_in_days       TINYINT      NOT NULL,                     -- 1..30, default 14
  expires_ms            BIGINT       NULL,                         -- WorkOS's expires_at, once sent
  resend_count          TINYINT      NOT NULL DEFAULT 0,           -- at most 3 (D1747)
  attempts              TINYINT      NOT NULL DEFAULT 0,           -- WorkOS create attempts by the queue
  accepted_user_id      VARCHAR(64)  NULL,                         -- WorkOS's accepted_user_id
  created_by            VARCHAR(64)  NOT NULL,                     -- the admin's WorkOS user id
  batch_key             VARCHAR(64)  NOT NULL,                     -- the Idempotency-Key of the batch that queued it
  created_ms            BIGINT       NOT NULL,
  updated_ms            BIGINT       NOT NULL,
  UNIQUE KEY uk_invites_workos (workos_invitation_id),
  UNIQUE KEY uk_invites_batch_entry (batch_key, waitlist_id),
  UNIQUE KEY uk_invites_open (open_key),                           -- at most one open invite per entry (NULLs don't collide)
  KEY k_invites_waitlist (waitlist_id),
  KEY k_invites_email (email_hmac),
  KEY k_invites_state_created (state, created_ms)
);

-- Loams Cloud accounts. workos_user_id is the access token's `sub`; WorkOS's
-- external_id holds our acct_ id.
CREATE TABLE IF NOT EXISTS accounts (
  id                  VARCHAR(40)  NOT NULL PRIMARY KEY,           -- acct_ + 128-bit random
  workos_user_id      VARCHAR(64)  NOT NULL,
  contact_email_enc   VARCHAR(512) NOT NULL,
  contact_email_hmac  CHAR(64)     NOT NULL,
  signin_email_hmac   CHAR(64)     NULL,                           -- when it differs from the invited address (D1742)
  invite_id           VARCHAR(40)  NOT NULL,
  status              VARCHAR(16)  NOT NULL,                       -- active | suspended | deleting
  created_ms          BIGINT       NOT NULL,
  updated_ms          BIGINT       NOT NULL,
  suspended_ms        BIGINT       NULL,
  deletion_due_ms     BIGINT       NULL,                           -- D1731: within 7 days of DELETE /me
  UNIQUE KEY uk_accounts_workos_user (workos_user_id),
  UNIQUE KEY uk_accounts_invite (invite_id),
  KEY k_accounts_contact (contact_email_hmac),
  KEY k_accounts_status (status, updated_ms)
);

-- The WorkOS events API cursor (D1754): one row per feed.
CREATE TABLE IF NOT EXISTS workos_events_cursor (
  feed        VARCHAR(16) NOT NULL PRIMARY KEY,                   -- 'workos'
  after       VARCHAR(64) NULL,                                   -- the last processed event id
  updated_ms  BIGINT      NOT NULL
);

-- Admin TOTP factors and step-ups (§25.8, D1746).
CREATE TABLE IF NOT EXISTS admin_mfa_factors (
  workos_user_id  VARCHAR(64) NOT NULL PRIMARY KEY,
  factor_id       VARCHAR(64) NOT NULL,
  created_ms      BIGINT      NOT NULL
);

CREATE TABLE IF NOT EXISTS admin_step_ups (
  sid               VARCHAR(64) NOT NULL PRIMARY KEY,              -- the AuthKit session id
  workos_user_id    VARCHAR(64) NOT NULL,
  step_up_until_ms  BIGINT      NOT NULL,                          -- verified + 15 minutes
  updated_ms        BIGINT      NOT NULL,
  KEY k_step_ups_until (step_up_until_ms)
);

-- Idempotency keys for API writes (Idempotency-Key, LC0 Task 3): a reused key
-- with a different request gets 409; the same request gets the stored response.
CREATE TABLE IF NOT EXISTS idempotency_keys (
  scope          VARCHAR(64)  NOT NULL,                            -- e.g. admin:<workos_user_id>:invites
  idem_key       VARCHAR(64)  NOT NULL,
  request_hash   CHAR(64)     NOT NULL,
  response_json  MEDIUMTEXT   NULL,
  created_ms     BIGINT       NOT NULL,
  PRIMARY KEY (scope, idem_key),
  KEY k_idem_created (created_ms)
);

-- The shared daily email budget (D1738, Q1288; R-LC2a-4: Resend Free sends 100 a
-- day across WorkOS's mail and ours). One row per UTC day and kind, plus a
-- 'total' row. A send reserves with a guarded increment; a retried increment
-- over-counts, which errs on the safe side.
CREATE TABLE IF NOT EXISTS email_budget (
  day         CHAR(10)    NOT NULL,                               -- YYYY-MM-DD, UTC
  kind        VARCHAR(16) NOT NULL,                               -- total | invite | waitlist
  n           INT         NOT NULL DEFAULT 0,
  updated_ms  BIGINT      NOT NULL,
  PRIMARY KEY (day, kind)
);

-- Audit events (D1748): CloudEvents under dev.loams.cloud.*, with ids and HMACs
-- as subjects and never a plaintext address. Kept 12 months.
CREATE TABLE IF NOT EXISTS audit (
  id         VARCHAR(40)  NOT NULL PRIMARY KEY,                    -- evt_ + 128-bit random
  time_ms    BIGINT       NOT NULL,
  type       VARCHAR(64)  NOT NULL,                                -- dev.loams.cloud.waitlist.joined, ...
  actor      VARCHAR(64)  NOT NULL,                                -- system | public | a WorkOS user id
  subject    VARCHAR(64)  NOT NULL,                                -- wl_, inv_, acct_ id, or an email HMAC
  detail     JSON         NULL,
  KEY k_audit_time (time_ms),
  KEY k_audit_subject_time (subject, time_ms),
  KEY k_audit_type_time (type, time_ms)
);

INSERT IGNORE INTO schema_migrations (version, name, applied_ms)
  VALUES (1, '0001_waitlist', CAST(UNIX_TIMESTAMP(NOW(3)) * 1000 AS SIGNED));

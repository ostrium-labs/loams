# Loams Cloud: threat model

> **Status:** draft for LC2a Task 12 (2026-10-24). Design: [§78 §25](../design/78-loams-cloud-on-tidb-x.md#25-invite-only-access-workos-authkit-and-the-sign-in-policy-d1736d1757-owner-amendments-of-2026-10-23) (D1736–D1758). Plan: [LC1, LC2a](../plans/2026-10-23-lc1-loams-cloud.md), rulings R-LC2a-1 to R-LC2a-19. Code: `deploy/loams-cloud/`, `crates/loams-cloud-admin/`, `deploy/openfga/core.fga`.
>
> **What exists today:** the control-DB schema (Task 1), the waitlist API (Task 3), admin authorisation (Task 4), invitations (Task 5), lifecycle and retention (Task 9), the admin CLI, and the Worker's wiring, all tested against local doubles. **Nothing is deployed.**
>
> **Not built yet, waiting on the owner's setup steps:** Task 2 (Resend email), Task 6 (WorkOS actions and the configuration checklist), Task 7 (sign-in, callback and redemption), Task 8 (the events reader and webhook), Task 10 (the pages, in loams-site) and Task 11 (end to end). Wherever a mitigation below depends on one of them, its row says so. This document is final only when those rows are closed.

## 1. Assets and actors

| Asset | Where it lives |
|---|---|
| Waitlist addresses | The control DB, as HMAC-SHA-256 (lookups) and AES-256-GCM (sending, the admin list); keys in Worker secrets |
| Confirm and removal tokens | Mailed in URL fragments; stored only as SHA-256 |
| Invitation tokens | WorkOS only. We keep `workos_invitation_id` and never read back the token or accept URL |
| Accounts | The control DB, keyed by `workos_user_id`; the contact address as HMAC plus AES-GCM |
| The admin API | `/cloud/v1/admin/*`: reads addresses, sends invitations, deletes entries |
| Secrets | Worker secrets: the control DB URL, the HMAC and AES keys, the Turnstile secret, the WorkOS API key; later the Resend key and the WorkOS actions and webhook secrets |
| The free tiers | Resend's 100 emails a day, Workers Free's 100,000 requests a day, Starter's request units |

| Actor | Can do |
|---|---|
| Anyone on the internet | Call the public endpoints; submit any address |
| A bot farm | Many addresses, many IP prefixes, solved or bought Turnstile tokens |
| An invitee | Forward or sell their invitation link |
| A person with a WorkOS dashboard login | Switch sign-in methods on, or change the actions' endpoints |
| An agent or service account | Hold a WorkOS token (M2M) or act for a person |
| A compromised WorkOS or Resend account | Read what that service holds; send mail as us |

Out of scope: a compromised Cloudflare account, a compromised TiDB Cloud organisation, and the browser.

## 2. Threats and mitigations

| # | Threat | Mitigation | Test | Status |
|---|---|---|---|---|
| T1 | **Enumeration**: learning whether an address is listed, confirmed or suppressed | `POST /waitlist` and `DELETE /waitlist` (by address) answer `202 {status:"check_your_email"}` for every state, with the same headers. The address-dependent work runs after the response (`waitUntil`), so timing doesn't depend on state either. Malformed input is a 400 that never depends on state. Only the inbox owner learns anything | `waitlist_responses_do_not_enumerate` | Built |
| T2 | **Waitlist flooding** | Turnstile on every address-taking call, verified server-side and single use. Per IP prefix (/24, /48): 5 per 10 minutes, 20 a day, counted globally in Durable Objects. At most 3 confirmation emails per address per day, and one "already listed" email a day. Disposable and no-MX domains are dropped silently. Unconfirmed entries are purged after 7 days | `turnstile_token_reused_is_refused`, `rate_limits_apply_per_prefix_and_address`, `waitlist_accepts_free_mail_but_not_disposable`, `retention_jobs_delete_on_schedule` | Built; the abuse drill is Task 11 |
| T3 | **Sending to someone else's address** (using us to mail a victim) | Double opt-in: nothing but the confirm email goes to an unconfirmed address, at most 3 a day. Every email carries a removal link, and removal suppresses the address for 24 months | `confirm_is_single_use`, the removal tests | Built; the email bodies and `List-Unsubscribe` are Task 2 |
| T4 | **Invitation forwarding or resale** | Each invitation creates at most one account (`accounts.invite_id` is unique, and the callback requires `accepted_user_id` equal to the user). Admins invite in batches they review. An invitation can be revoked at WorkOS | `workos_user_id_is_unique`, `revoke_mirrors_workos_state` | Partly built; the callback check is Task 7 |
| T5 | **A sign-in method switched on by mistake** in the dashboard | Four layers (D1752): the AuthKit configuration; the registration and authentication actions (fail closed), which allow only `GoogleOAuth`, `GitHubOAuth`, `MicrosoftOAuth` and `MagicAuth` (R-LC2a-5.4); the callback re-check; reconciliation that suspends and revokes on any other success | The policy suite in `signin_policy.test.ts` | **Not built** (Tasks 6–8). The domain rules for email codes exist (`emailCodeDomainRefusal`) |
| T6 | **An actions outage** | WorkOS actions are configured fail-closed, so an outage denies sign-in; reconciliation catches a fail-open mistake | `reconciliation_catches_dashboard_mistake` | **Not built** (Tasks 6, 8) |
| T7 | **Webhook forgery and replay** | `WorkOS-Signature` verified with a 5-minute tolerance and WorkOS's source IPs; the webhook only wakes the events reader, which reads the signed events API from its cursor | `webhook_rejects_bad_signature_and_replay`, `wake_is_only_a_hint` | **Not built** (Task 8; needs the webhook secret) |
| T8 | **Admin takeover** | Every `/admin/*` call needs a valid AuthKit token for a person's session, in the Ostrium organisation with `cloud_admin`; the user in `config/admins.toml` (a reviewed PR); and a TOTP step-up within 15 minutes, held per session id. Tokens without a session, with a non-user subject, or with `act`/`may_act` are refused before the role is read. `platform:loams-cloud#cloud_admin` admits users only. 60 admin calls a minute; 5 TOTP tries per 5 minutes. Every admin action and every read of the list is audited | `admin_requires_role_allowlist_and_step_up`, `step_up_expires_after_15_minutes`, `agents_cannot_be_cloud_admin`, the OpenFGA model tests | Built |
| T9 | **A stolen admin laptop** | The CLI keeps only the refresh token, in the OS keychain; every command needs a fresh step-up after 15 minutes; `loams cloud admin logout` removes it; the owner can remove the admin from the allow-list or the WorkOS organisation | The CLI's tests | Built |
| T10 | **WorkOS account compromise** | WorkOS holds identity only: no passwords exist (D1757). An attacker could invite or sign in as anyone; our allow-list and step-up still gate the admin API, and our callback still requires our invite rows. Rotate the API key; review the audit | — | Partly (Task 7 for the callback) |
| T11 | **Resend account compromise** | The Worker gets a sending-only key restricted to the domain (R-LC2a-2). The full-access key stays only in WorkOS's provider settings. Open and click tracking off, so tokens never reach Resend's logs (R-LC2a-3) | — | **Owner steps** (R-LC2a-2, R-LC2a-3) |
| T12 | **A control-DB dump** | It holds no readable address: HMACs and AES-GCM only, keys in Worker secrets. No invitation token. Tokens as SHA-256 | `no_plaintext_email_column_outside_enc`, `tokens_are_redacted_from_logs` | Built |
| T13 | **Addresses or tokens in logs** | Every log line goes through redaction (tokens, addresses, bearer values, fragments); logs carry the path only, never the query. The audit table refuses an address | `tokens_are_redacted_from_logs` | Built |
| T14 | **Retried or aborted writes** (LC0: aborted writes commit) | Every write is one statement safe to repeat; API writes carry `Idempotency-Key`; a WorkOS create is adopted, never repeated | `invite_calls_workos_once_per_entry`, `email_hmac_is_unique` | Built |
| T15 | **Data-subject requests** | Waitlist removal by link or by address; `DELETE /me` deletes the WorkOS user and our records within 7 days, keeping only suppression HMACs. WorkOS events keep a deleted user's email for up to 90 days (R-LC2a-8), which the notice states | `deletion_leaves_only_hmac_and_deletes_workos_user` | Built (the export step joins in LC2) |
| T16 | **Free-tier exhaustion as denial of service** | The email budget: 100 a day in all, at most 60 invitations, counted in the control DB; batches that don't fit are refused. Rate limits bound waitlist requests. Workers Free fails closed past its daily limits, until 00:00 UTC. WorkOS's magic codes are uncounted by us (R-LC2a-17) | `daily_budget_blocks_batch`, `pacer_shares_daily_budget_with_invitations` | Built; the measured week is Task 11 |
| T17 | **Supply chain** | `deploy/loams-cloud` has one runtime dependency (`@tidbcloud/serverless` 0.3.0, LC0's pin) and a lockfile. The domain list is vendored at a pinned revision with its SHA-256 checked in a test | `the vendored lists load, at their pins` | Built |

## 3. Go-live checklist (the owner ticks)

Nothing goes live until every box is ticked.

- [ ] Counsel's privacy policy is published (Q1276), and the notice beside the form names WorkOS, Resend, Cloudflare and TiDB Cloud.
- [ ] Resend: the sending domain is verified; open and click tracking are off (R-LC2a-3); the Worker's key is sending-only (R-LC2a-2); DMARC moves from `p=none` once Resend, Brevo and Email Routing are aligned (R-LC2a-1).
- [ ] WorkOS production matches `deploy/loams-cloud/workos/CHECKLIST.md` (Task 6) and passes `loams cloud admin check-workos`; impersonation is off (R-LC2a-6).
- [ ] The first admin is in `config/admins.toml`, in the Ostrium organisation with `cloud_admin`, and enrolled in TOTP.
- [ ] The daily caps are set (`TOTAL_PER_DAY`, `INVITES_PER_DAY` in `src/budget.ts`), with a margin for WorkOS's magic codes.
- [ ] Worker secrets are set, each newly generated for production; the HMAC and AES keys are kept in a second place, since losing them makes the stored addresses unreadable.
- [ ] Tasks 2, 6, 7, 8 and 10 are merged, and Task 11's end-to-end run and abuse drill pass on staging.

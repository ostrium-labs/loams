/**
 * Zulip wire types.
 *
 * Verified against Zulip `12.0-dev+git`, `API_FEATURE_LEVEL = 512`. Everything
 * below is a field that exists on the wire in that version. Where a field is
 * only present under a condition (a permission, a query parameter, a message
 * type) it is modelled as optional and the condition is named, because an
 * always-present type for a conditional field is exactly how `undefined`
 * reaches a chart.
 *
 * THE ENVELOPE
 * ------------
 * Every Zulip API response — success AND failure — carries a `result`
 * discriminator and a human-readable `msg`. Data fields sit FLAT alongside
 * `result`/`msg`; there is no `data` wrapper. See {@link ZulipSuccess} and
 * {@link ZulipError}.
 *
 * `ignored_parameters_unsupported` deserves its own note. Since feature level
 * 167 Zulip reports, on every response, which of the parameters you sent it does
 * not know. It is the cheapest possible guard against a silently wrong
 * interface: if this adapter ever sends a misspelled or removed parameter, the
 * server says so in this field instead of quietly ignoring it. The adapter logs
 * it on every single response; see `ZulipAdapterService`.
 */

/* -------------------------------------------------------------------------- */
/* Envelope                                                                    */
/* -------------------------------------------------------------------------- */

/** A successful response: the envelope fields plus whatever data came back. */
export type ZulipSuccess<T> = {
  result: "success";
  msg: string;
  /**
   * Parameter names this server version does not support and therefore dropped.
   * Present since feature level 167. Read on EVERY response and logged — see the
   * module comment.
   */
  ignored_parameters_unsupported?: string[];
} & T;

/** A failure response. `code` is set for a few machine-actionable cases only. */
export interface ZulipError {
  result: "error";
  msg: string;
  /** e.g. `RATE_LIMIT_HIT`, `REQUEST_VARIABLE_MISSING`. Often absent. */
  code?: string;
}

/** What `GET /messages` answers with. */
export interface ZulipSuccessEnvelope {
  result: "success";
  msg: string;
  ignored_parameters_unsupported?: string[];
}

/** A rate-limited or otherwise rejected request: 429 with `code: RATE_LIMIT_HIT`. */
export type ZulipRateLimitHit = ZulipError & { code: "RATE_LIMIT_HIT" };

/* -------------------------------------------------------------------------- */
/* Users                                                                       */
/* -------------------------------------------------------------------------- */

/**
 * A realm user, as returned by `GET /users/{id}` (under `user`).
 *
 * Fields whose visibility is permission-dependent are optional on purpose:
 * `email`, `delivery_email` and `is_admin` are only visible to self or an
 * admin, and `profile_data` only to users allowed to see it.
 */
export interface ZulipUser {
  user_id: number;
  full_name: string;
  avatar_url?: string | null;
  /** `is_active` is Zulip's term for "not deactivated". */
  is_active: boolean;
  is_bot: boolean;
  is_guest: boolean;
  is_admin?: boolean;
  /** Primary email. Visible only to self / admins / with permission. */
  email?: string;
  /**
   * The address used for API-key Basic auth, matched case-insensitively against
   * `user_profile.delivery_email`. This is NOT `email` and NOT a user id.
   */
  delivery_email?: string;
  /**
   * Custom profile field values keyed by field id. The values themselves are
   * encoded per field; for dropdown fields the value is a JSON-encoded STRING
   * that must be `JSON.parse`d. See {@link ZulipCustomField.field_data}.
   */
  profile_data?: Record<string, string>;
  /** Anything this adapter does not model. */
  [key: string]: unknown;
}

/**
 * The self profile from `GET /users/me`.
 *
 * TRAP: the user fields are spread FLAT at the top level — there is no `user`
 * key here, unlike `GET /users/{id}`. The one addition is `max_message_id`.
 */
export type ZulipSelfProfile = ZulipUser & {
  /**
   * The highest message id that exists in the realm. An UPPER BOUND on the
   * message count, not a count: message ids have gaps (deletions, other realms'
   * history), so it over-reports.
   */
  max_message_id: number;
};

/* -------------------------------------------------------------------------- */
/* Streams ("channels" in prose, `streams` on the wire)                        */
/* -------------------------------------------------------------------------- */

/**
 * A stream.
 *
 * TRAP: the id field is `stream_id`, not `id`. Zulip 12.x still says "stream"
 * on the wire despite calling them channels in its UI prose.
 */
export interface ZulipStream {
  stream_id: number;
  name: string;
  description: string;
  invite_only: boolean;
  /**
   * Only present when `GET /streams` was called with `include_default=true`.
   * There is NO `include_default_subscriptions` parameter — asking for that
   * name yields an `ignored_parameters_unsupported` entry.
   */
  is_default?: boolean;
  message_retention_days?: number | null;
  history_retention_days?: number | null;
}

/** `GET /users/me/{stream_id}/topics` — one request per stream, no pagination. */
export interface ZulipTopic {
  name: string;
  /** Highest message id in the topic. The list is sorted by this, DESCENDING. */
  max_id: number;
}

/* -------------------------------------------------------------------------- */
/* Narrow and messages                                                         */
/* -------------------------------------------------------------------------- */

/**
 * Narrow operators this adapter uses.
 *
 * Legacy 2-tuple narrows (`["channel", 5]`) still parse server-side but CANNOT
 * express `negated`, so this adapter always sends the object form.
 */
export type ZulipNarrowOperator =
  | "search"
  | "sender"
  | "has"
  | "channel"
  | "topic"
  | "id"
  | "near"
  | "is"
  | "dm"
  | "dm-including";

/** One term of a narrow. */
export interface ZulipNarrowTerm {
  operator: ZulipNarrowOperator;
  /** Operator-specific. `search` takes the query string; `has` takes a type. */
  operand: string | number | boolean;
  /** Inverts the term. Only expressible in the object form. */
  negated?: boolean;
}

/**
 * `has` operands.
 *
 * TRAP: it is `link`, `attachment`, `image`, `reaction` — `file` and `photo`
 * do not exist.
 */
export type ZulipHasOperand = "link" | "attachment" | "image" | "reaction";

/**
 * What `type` can be on a RESPONSE.
 *
 * TRAP: responses only ever use `stream` and `private`. `channel` and `direct`
 * are accepted on a request but are never returned.
 */
export type ZulipMessageType = "stream" | "private";

/** A message flag injected by the server after the schema runs. */
export type ZulipMessageFlag =
  | "read"
  | "historical"
  | "starred"
  | "collapsed"
  | "mentioned"
  | "wildcard_mentioned"
  | "stream_wildcard_mentioned"
  | "topic_wildcard_mentioned"
  | "has_alert_word";

/** An inline reaction. */
export interface ZulipReaction {
  emoji_name: string;
  emoji_code: string;
  /** `unicode_emoji`, `realm_emoji`, or `zulip_extra_emoji`. */
  reaction_type: string;
  user_id: number;
}

/** A user summary inside a DM's `display_recipient`. */
export interface ZulipRecipient {
  id: number;
  email: string;
  full_name: string;
  [key: string]: unknown;
}

/** One message. */
export interface ZulipMessage {
  id: number;
  /** Only `stream` and `private` appear in responses. */
  type: ZulipMessageType;
  sender_id: number;
  sender_email: string;
  sender_full_name: string;
  sender_realm_str: string;
  avatar_url?: string | null;
  /** Rendered Markdown HTML. */
  content: string;
  content_type: string;
  /** UNIX SECONDS, not milliseconds and not ISO 8601. */
  timestamp: number;
  recipient_id: number;
  /**
   * UNION, and this is a real trap:
   * - for a channel message: the bare channel name, a STRING
   * - for a DM: an ARRAY of user objects
   */
  display_recipient: string | ZulipRecipient[];
  /**
   * The message's TOPIC. The field is named `subject` because of Zulip's
   * pre-2016 history; it is not a subject line distinct from the topic.
   */
  subject: string;
  /** Present on channel messages only. */
  stream_id?: number;
  client?: string;
  is_me_message?: boolean;
  flags?: ZulipMessageFlag[];
  /**
   * Reactions are INLINE. There is no reaction-listing endpoint in Zulip; this
   * array is the only way to read them.
   */
  reactions?: ZulipReaction[];
  submessages?: unknown[];
  topic_links?: unknown[];
  /** Present only when a `search` narrow matched. */
  match_content?: boolean;
  /** Present only when a `search` narrow matched the TOPIC. */
  match_subject?: boolean;
  [key: string]: unknown;
}

/**
 * A `GET /messages` page.
 *
 * PAGINATION IS MESSAGE-ID ANCHORING. There are no totals, no page numbers and
 * no `has_more`. The loop is: take `anchor`, ask for `num_before` messages
 * before it, and feed the returned `anchor` back in until `found_oldest` is
 * true.
 */
export interface ZulipMessagesPage {
  messages: ZulipMessage[];
  /** Whether `anchor` itself was found. */
  found_anchor: boolean;
  /** Whether the page reached the start of the accessible history. Loop until true. */
  found_oldest: boolean;
  /** Whether the page reached the newest message. */
  found_newest: boolean;
  /** Whether the user's access is limited to post-join history. */
  history_limited: boolean;
  /**
   * The anchor to use for the next `num_before` request.
   *
   * ABSENT from the `message_ids` variant of `GET /messages`, which returns
   * `found_anchor`/`found_oldest`/`found_newest` all false.
   */
  anchor?: number;
}

/**
 * What `anchor` accepts.
 *
 * `date` is new in Zulip 12.0 and requires `anchor_date` (ISO 8601). It is the
 * natural "everything since X" primitive and the only bulk time-window read the
 * API offers.
 *
 * WIRE NAME: the Python parameter is `anchor_val` and `ApiParamConfig` renames
 * it to `anchor` on the wire. `client_requested_message_ids` is likewise renamed
 * to `message_ids`. Send the wire names.
 */
export type ZulipAnchor = number | "newest" | "oldest" | "first_unread" | "date";

/** Parameters for one `GET /messages` page. */
export interface ZulipMessagesQuery {
  narrow?: ZulipNarrowTerm[];
  anchor?: ZulipAnchor;
  /** ISO 8601. Required when `anchor` is `date`. */
  anchor_date?: string;
  num_before?: number;
  num_after?: number;
  /** Mutually exclusive with anchor / num_before / num_after. */
  message_ids?: number[];
}

/**
 * `GET /messages/matches_narrow`.
 *
 * TRAP: this is a MEMBERSHIP TEST ("which of these messages match this
 * narrow?"), not a search. Full-text search is `GET /messages` with a
 * `search` narrow.
 *
 * The result is an object keyed by message id as a STRING, and the per-match
 * object is `{match_content, match_subject}` — `match_subject`, NOT
 * `match_topic`.
 */
export type ZulipNarrowMatches = Record<string, { match_content: boolean; match_subject: boolean }>;

/* -------------------------------------------------------------------------- */
/* Realm metadata                                                              */
/* -------------------------------------------------------------------------- */

/**
 * A custom profile field from `GET /realm/profile_fields`.
 *
 * TRAP: `field_data` is a JSON-ENCODED STRING, not an object. For a dropdown
 * field it decodes to `{"field_data": "<value>"}`. Parse it with
 * {@link parseProfileFieldData}; a `JSON.parse` on it that expects an object
 * without decoding the string is the classic version of this bug.
 */
export interface ZulipCustomField {
  id: number;
  type: number;
  name: string;
  hint?: string;
  /** JSON-encoded string. See the note above. */
  field_data: string;
  order?: number;
  display_in_profile_summary?: boolean;
  [key: string]: unknown;
}

/** A realm custom emoji, from `GET /realm/emoji`. */
export interface ZulipEmoji {
  /** A STRING even though the map key is the same id. */
  id: string;
  name: string;
  source_url: string;
  deactivated: boolean;
  author_id: number | null;
  /** Last URL the emoji was reachable at, kept after deactivation. */
  still_url?: string;
}

/* -------------------------------------------------------------------------- */
/* Presence                                                                    */
/* -------------------------------------------------------------------------- */

/** A modern presence entry, keyed by STRINGIFIED user id. */
export interface ZulipPresenceModern {
  active_timestamp: number;
  idle_timestamp: number;
}

/** A legacy aggregated presence entry, keyed by delivery email. */
export interface ZulipPresenceAggregated {
  client: string;
  status: string;
  timestamp: number;
}

/** A legacy website presence entry, keyed by delivery email. */
export interface ZulipPresenceWebsite {
  client?: string;
  status: string;
  timestamp: number;
  [key: string]: unknown;
}

/** The legacy half of the presence key space. */
export type ZulipPresenceLegacy = ZulipPresenceAggregated | ZulipPresenceWebsite;

/**
 * A presence entry from `GET /users/{id_or_email}/presence`.
 *
 * TRAP: `presence` MERGES TWO KEY SPACES in one object. Modern entries are keyed
 * by the user id STRINGIFIED (`{"3001": {...}}`); legacy entries are keyed by
 * delivery email (`{"ada@example.com": {...}}`). Use {@link isModernPresence} to
 * tell them apart instead of trusting the key shape.
 *
 * Bots are REJECTED by this endpoint with a 400, not a 401.
 */
export type ZulipPresenceEntry = ZulipPresenceModern | ZulipPresenceLegacy;

/** `GET /users/{id_or_email}/presence`. */
export interface ZulipUserPresence {
  presence: Record<string, ZulipPresenceEntry>;
  server_timestamp: number;
}

/** `GET /realm/presence`. */
export interface ZulipRealmPresence {
  presences: Record<string, ZulipPresenceEntry>;
  server_timestamp: number;
}

/* -------------------------------------------------------------------------- */
/* Rate limiting                                                               */
/* -------------------------------------------------------------------------- */

/**
 * Zulip's rate-limit headers.
 *
 * TRAP: `X-RateLimit-Reset` is a UNIX TIMESTAMP in seconds, NOT a delta and NOT
 * milliseconds.
 *
 * The limit is 200 requests / 60 s per authenticated user, shared across ALL
 * endpoints — there is no higher tier for read-only traffic — and 100 / 60 s per
 * IP unauthenticated. The adapter therefore self-throttles off
 * `X-RateLimit-Remaining` rather than waiting to be 429'd.
 */
export interface ZulipRateLimitState {
  limit: number;
  remaining: number;
  /** UNIX seconds. */
  reset: number;
  observedAt: number;
}

/* -------------------------------------------------------------------------- */
/* Derived metrics                                                             */
/* -------------------------------------------------------------------------- */

/**
 * Metrics DERIVED from ordinary reads.
 *
 * Zulip has NO aggregate or statistics endpoint. There is no
 * `realm_daily_active_users`, no DAU, no MAU, nothing equivalent — an attempt
 * to read one returns 404 and the name does not exist anywhere in the server
 * tree. Anything presented as a Zulip DAU figure is fabricated.
 *
 * What is legitimately derivable is below. Every field names the endpoint it
 * came from, because the difference between "measured" and "approximated by an
 * upper bound" is the whole point of recording it.
 */
export interface ZulipDerivedMetrics {
  /**
   * UPPER BOUND on the number of messages in the realm, from
   * `GET /users/me` → `max_message_id`. NOT a count: ids have gaps.
   */
  messageIdUpperBound: number;
  /** `GET /streams` → `streams.length`. */
  streamCount: number;
  /** `GET /users` → count over `members[]`. */
  activeHumanUsers: number;
  botCount: number;
  guestCount: number;
  adminCount: number;
  deactivatedUserCount: number;
  /**
   * Every field above with the caveat spelled out, so a caller that renders a
   * number cannot forget the caveat.
   */
  caveats: readonly string[];
}

/* -------------------------------------------------------------------------- */
/* Config                                                                      */
/* -------------------------------------------------------------------------- */

export interface ZulipConfig {
  /** The realm root, e.g. `https://chat.example.com`. `/api/v1` is appended. */
  baseUrl: string;
  /**
   * The account's DELIVERY EMAIL.
   *
   * This must match `user_profile.delivery_email` (compared
   * case-insensitively). Not `email`, not a user id. A correct API key with the
   * wrong username is a 401, and the error does not distinguish the two cases.
   */
  email: string;
  /** The API key. Created by the user in the Zulip UI. */
  apiKey: string;
  /** Per-request timeout. Defaults to 30s via the shared client. */
  timeoutMs?: number;
  /**
   * Pause once `X-RateLimit-Remaining` drops to this value, until the reset
   * instant. Defaults to 10.
   */
  rateLimitFloor?: number;
  /** Hard stop for {@link ZulipAdapterService.fetchAllMessages}. Defaults to 50. */
  maxPages?: number;
}

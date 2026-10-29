// WorkOS's REST API, over fetch (design 78 §25.8–§25.10). The interface is what
// the control plane needs; HttpWorkOs is the real client, and
// test/harness/workos.ts is the recorded fake the suites use. No test calls
// WorkOS: the staging run is LC2a Task 11's, with the owner's staging key.
//
// MFA (R-LC2a-5.7): AuthKit's user-scoped factors API,
// POST /user_management/users/{id}/auth_factors, then the MFA API's
// challenge and verify, which are free inside AuthKit.

export interface TotpEnrolment {
  factorId: string;
  qrCode: string;
  secret: string;
  uri: string;
}

export interface WorkOsMfa {
  enrollTotp(userId: string, issuer: string, user: string): Promise<TotpEnrolment>;
  challenge(factorId: string): Promise<string>;
  verify(challengeId: string, code: string): Promise<boolean>;
}

/**
 * An application-wide invitation (§25.1: no organisation). We keep only `id`,
 * `state`, `expires_at` and `accepted_user_id`; WorkOS's `token` and
 * `accept_invitation_url` are dropped on read and never stored (D1739).
 */
export interface Invitation {
  id: string;
  state: "pending" | "accepted" | "expired" | "revoked";
  expiresAt: number;
  acceptedUserId: string | null;
}

export interface WorkOsInvitations {
  createInvitation(email: string, expiresInDays: number): Promise<Invitation>;
  /** The pending invitation for this address, if one exists: lets a retried create adopt it. */
  findPendingInvitation(email: string): Promise<Invitation | null>;
  getInvitation(id: string): Promise<Invitation>;
  revokeInvitation(id: string): Promise<Invitation>;
  /** Resends the email; WorkOS keeps the token and the expiry (R-LC2a-5.6). */
  resendInvitation(id: string): Promise<Invitation>;
}

interface RawInvitation {
  id: string;
  email?: string;
  state: Invitation["state"];
  expires_at: string;
  accepted_user_id?: string | null;
}

function invitationOf(r: RawInvitation): Invitation {
  return { id: r.id, state: r.state, expiresAt: Date.parse(r.expires_at), acceptedUserId: r.accepted_user_id ?? null };
}

export interface WorkOsUsers {
  /** Deletes a WorkOS user permanently (D1731, D1757). Already gone is not an error. */
  deleteUser(id: string): Promise<void>;
}

export class WorkOsError extends Error {
  readonly status: number;
  readonly code?: string;
  constructor(status: number, message: string, code?: string) {
    super(message);
    this.status = status;
    this.code = code;
  }
}

export class HttpWorkOs implements WorkOsMfa, WorkOsInvitations, WorkOsUsers {
  private readonly apiKey: string;
  private readonly base: string;
  private readonly fetchFn: typeof fetch;
  constructor(apiKey: string, opts: { base?: string; fetch?: typeof fetch } = {}) {
    this.apiKey = apiKey;
    this.base = opts.base ?? "https://api.workos.com";
    this.fetchFn = opts.fetch ?? fetch;
  }

  /** One request. Writes carry an Idempotency-Key when the caller has one (LC0: aborted writes commit). */
  async request<T>(method: string, path: string, body?: unknown, idempotencyKey?: string): Promise<T> {
    const headers: Record<string, string> = { authorization: `Bearer ${this.apiKey}`, accept: "application/json" };
    if (body !== undefined) headers["content-type"] = "application/json";
    if (idempotencyKey) headers["idempotency-key"] = idempotencyKey;
    const r = await this.fetchFn(this.base + path, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(10_000),
    });
    const text = await r.text();
    let parsed: unknown = undefined;
    try {
      parsed = text ? JSON.parse(text) : undefined;
    } catch {
      // not JSON
    }
    if (!r.ok) {
      const p = (parsed ?? {}) as { code?: string; message?: string; error?: string };
      throw new WorkOsError(r.status, `WorkOS ${method} ${path.replace(/\/[^/]*_[A-Z0-9]{10,}[^/]*/g, "/{id}")}: ${r.status} ${p.code ?? p.error ?? ""}`.trim(), p.code ?? p.error);
    }
    return parsed as T;
  }

  async enrollTotp(userId: string, issuer: string, user: string): Promise<TotpEnrolment> {
    const r = await this.request<{ authentication_factor: { id: string; totp: { qr_code: string; secret: string; uri: string } } }>(
      "POST", `/user_management/users/${encodeURIComponent(userId)}/auth_factors`, { type: "totp", totp_issuer: issuer, totp_user: user },
    );
    const f = r.authentication_factor;
    return { factorId: f.id, qrCode: f.totp.qr_code, secret: f.totp.secret, uri: f.totp.uri };
  }

  async challenge(factorId: string): Promise<string> {
    const r = await this.request<{ id: string }>("POST", `/auth/factors/${encodeURIComponent(factorId)}/challenge`, {});
    return r.id;
  }

  async verify(challengeId: string, code: string): Promise<boolean> {
    try {
      const r = await this.request<{ valid: boolean }>("POST", `/auth/challenges/${encodeURIComponent(challengeId)}/verify`, { code });
      return r.valid === true;
    } catch (e) {
      // An expired or already-verified challenge is a failed verification, not an outage.
      if (e instanceof WorkOsError && (e.status === 400 || e.status === 422)) return false;
      throw e;
    }
  }

  async createInvitation(email: string, expiresInDays: number): Promise<Invitation> {
    return invitationOf(await this.request<RawInvitation>("POST", "/user_management/invitations", { email, expires_in_days: expiresInDays }));
  }

  async findPendingInvitation(email: string): Promise<Invitation | null> {
    const r = await this.request<{ data: RawInvitation[] }>("GET", `/user_management/invitations?email=${encodeURIComponent(email)}&limit=100`);
    const pending = r.data.filter((i) => i.state === "pending" && (i.email ?? email).toLowerCase() === email.toLowerCase());
    return pending.length ? invitationOf(pending[0]) : null;
  }

  async getInvitation(id: string): Promise<Invitation> {
    return invitationOf(await this.request<RawInvitation>("GET", `/user_management/invitations/${encodeURIComponent(id)}`));
  }

  async revokeInvitation(id: string): Promise<Invitation> {
    return invitationOf(await this.request<RawInvitation>("POST", `/user_management/invitations/${encodeURIComponent(id)}/revoke`, {}));
  }

  async resendInvitation(id: string): Promise<Invitation> {
    return invitationOf(await this.request<RawInvitation>("POST", `/user_management/invitations/${encodeURIComponent(id)}/resend`, {}));
  }

  async deleteUser(id: string): Promise<void> {
    try {
      await this.request<unknown>("DELETE", `/user_management/users/${encodeURIComponent(id)}`);
    } catch (e) {
      if (e instanceof WorkOsError && e.status === 404) return;
      throw e;
    }
  }
}

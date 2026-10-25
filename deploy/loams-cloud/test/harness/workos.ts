// A recorded fake of the WorkOS pieces the control plane calls, and an AuthKit
// token issuer with its own RSA key and JWKS. Nothing here reaches WorkOS.

import { JwtVerifier, type Jwk } from "../../src/jwt.ts";
import { WorkOsError, type Invitation, type TotpEnrolment, type WorkOsInvitations, type WorkOsMfa } from "../../src/workos.ts";

const enc = new TextEncoder();

function b64url(b: Uint8Array | string): string {
  const bytes = typeof b === "string" ? enc.encode(b) : b;
  let s = "";
  for (const x of bytes) s += String.fromCharCode(x);
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export const ISSUER = "https://api.workos.com/user_management/client_TEST0000000000";
export const OSTRIUM_ORG = "org_01OSTRIUM00000000000000";

export class TokenIssuer {
  readonly key: CryptoKeyPair;
  readonly kid: string;
  readonly jwk: Jwk;
  private constructor(key: CryptoKeyPair, kid: string, jwk: Jwk) {
    this.key = key;
    this.kid = kid;
    this.jwk = jwk;
  }

  static async create(kid = "key_1"): Promise<TokenIssuer> {
    const key = (await crypto.subtle.generateKey(
      { name: "RSASSA-PKCS1-v1_5", modulusLength: 2048, publicExponent: new Uint8Array([1, 0, 1]), hash: "SHA-256" },
      true,
      ["sign", "verify"],
    )) as CryptoKeyPair;
    const pub = (await crypto.subtle.exportKey("jwk", key.publicKey)) as JsonWebKey;
    return new TokenIssuer(key, kid, { kty: "RSA", kid, alg: "RS256", use: "sig", n: pub.n, e: pub.e });
  }

  async sign(claims: Record<string, unknown>, header: Record<string, unknown> = {}): Promise<string> {
    const h = b64url(JSON.stringify({ alg: "RS256", typ: "JWT", kid: this.kid, ...header }));
    const p = b64url(JSON.stringify(claims));
    const sig = new Uint8Array(await crypto.subtle.sign("RSASSA-PKCS1-v1_5", this.key.privateKey, enc.encode(`${h}.${p}`)));
    return `${h}.${p}.${b64url(sig)}`;
  }

  /** An admin's access token, valid for 5 minutes from `nowMs`. */
  adminToken(nowMs: number, over: Record<string, unknown> = {}): Promise<string> {
    const iat = Math.floor(nowMs / 1000);
    return this.sign({ iss: ISSUER, sub: "user_01ADMIN000000000000000", sid: "session_01ADMIN0000000000", org_id: OSTRIUM_ORG, role: "cloud_admin", roles: ["cloud_admin"], permissions: [], iat, exp: iat + 300, ...over });
  }

  verifier(now: () => number, keys: () => Jwk[] = () => [this.jwk]): JwtVerifier {
    return new JwtVerifier(async () => ({ keys: keys() }), { issuer: ISSUER, now });
  }
}

/** WorkOS's TOTP factors: each factor's current code is `codes.get(factorId)`. */
export class FakeMfa implements WorkOsMfa {
  private n = 0;
  readonly codes = new Map<string, string>();
  readonly challenges = new Map<string, { factorId: string; used: boolean }>();
  readonly calls: string[] = [];

  async enrollTotp(userId: string, issuer: string, user: string): Promise<TotpEnrolment> {
    this.calls.push(`enroll ${userId}`);
    const factorId = `auth_factor_${String(++this.n).padStart(20, "0")}`;
    this.codes.set(factorId, "246810");
    return { factorId, qrCode: "data:image/png;base64,AAAA", secret: "JBSWY3DPEHPK3PXP", uri: `otpauth://totp/${issuer}:${user}?secret=JBSWY3DPEHPK3PXP` };
  }
  async challenge(factorId: string): Promise<string> {
    this.calls.push(`challenge ${factorId}`);
    const id = `auth_challenge_${String(++this.n).padStart(20, "0")}`;
    this.challenges.set(id, { factorId, used: false });
    return id;
  }
  async verify(challengeId: string, code: string): Promise<boolean> {
    this.calls.push(`verify ${challengeId}`);
    const c = this.challenges.get(challengeId);
    if (!c || c.used) return false;
    c.used = true;
    return this.codes.get(c.factorId) === code;
  }
}

/**
 * WorkOS's invitations, recorded: application-wide, `pending` until accepted,
 * revoked or expired; resend keeps the token and expiry (R-LC2a-5.6); revoke and
 * resend are refused unless pending. `loseNextCreateResponse` creates the
 * invitation and then throws, as a lost response would.
 */
export class FakeInvitations implements WorkOsInvitations {
  private n = 0;
  readonly invitations = new Map<string, { id: string; email: string; state: Invitation["state"]; expiresAt: number; acceptedUserId: string | null; resent: number }>();
  readonly calls: string[] = [];
  readonly now: () => number;
  failNextCreate = 0;
  loseNextCreateResponse = false;
  constructor(now: () => number) {
    this.now = now;
  }

  private view(id: string): Invitation {
    const i = this.invitations.get(id);
    if (!i) throw new WorkOsError(404, "WorkOS: 404 entity_not_found", "entity_not_found");
    if (i.state === "pending" && i.expiresAt <= this.now()) i.state = "expired";
    return { id: i.id, state: i.state, expiresAt: i.expiresAt, acceptedUserId: i.acceptedUserId };
  }

  createdFor(email: string): number {
    return [...this.invitations.values()].filter((i) => i.email === email).length;
  }

  async createInvitation(email: string, expiresInDays: number): Promise<Invitation> {
    this.calls.push(`create ${email}`);
    if (this.failNextCreate > 0) {
      this.failNextCreate--;
      throw new WorkOsError(503, "WorkOS POST /user_management/invitations: 503");
    }
    const id = `invitation_${String(++this.n).padStart(20, "0")}`;
    this.invitations.set(id, { id, email, state: "pending", expiresAt: this.now() + expiresInDays * 86_400_000, acceptedUserId: null, resent: 0 });
    if (this.loseNextCreateResponse) {
      this.loseNextCreateResponse = false;
      throw new Error("The operation was aborted due to timeout");
    }
    return this.view(id);
  }
  async findPendingInvitation(email: string): Promise<Invitation | null> {
    this.calls.push(`find ${email}`);
    for (const i of this.invitations.values()) if (i.email === email && this.view(i.id).state === "pending") return this.view(i.id);
    return null;
  }
  async getInvitation(id: string): Promise<Invitation> {
    this.calls.push(`get ${id}`);
    return this.view(id);
  }
  async revokeInvitation(id: string): Promise<Invitation> {
    this.calls.push(`revoke ${id}`);
    if (this.view(id).state !== "pending") throw new WorkOsError(400, "WorkOS: 400 invite_not_pending", "invite_not_pending");
    this.invitations.get(id)!.state = "revoked";
    return this.view(id);
  }
  async resendInvitation(id: string): Promise<Invitation> {
    this.calls.push(`resend ${id}`);
    if (this.view(id).state !== "pending") throw new WorkOsError(400, "WorkOS: 400 invite_not_pending", "invite_not_pending");
    this.invitations.get(id)!.resent++;
    return this.view(id);
  }
  /** The person accepts in AuthKit. */
  accept(id: string, userId: string): void {
    const i = this.invitations.get(id)!;
    i.state = "accepted";
    i.acceptedUserId = userId;
  }
}

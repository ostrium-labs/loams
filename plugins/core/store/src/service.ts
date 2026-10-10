import { Context, Service } from "cordis";
import type { Pool } from "pg";

export interface StoreConfig {
  connectionString: string;
}

declare module "cordis" {
  interface Context {
    store: StoreService;
  }
}

export class StoreService extends Service {
  public pool?: Pool;
  public config: StoreConfig;
  private _memoryStore: Map<string, any> = new Map();
  private _memoryVersions: Map<string, any[]> = new Map();
  private _memoryAudit: any[] = [];
  private _memoryPluginState: Map<string, boolean> = new Map();
  private _memoryAuthSessions: Map<string, any> = new Map();
  private _memoryServiceTokens: Map<string, any> = new Map();
  public isMemoryMode = false;

  constructor(ctx: Context, config: StoreConfig) {
    super(ctx, "store");
    this.config = config;
    if (!config?.connectionString || config.connectionString.startsWith("memory")) {
      this.isMemoryMode = true;
    }
  }

  async [Service.init]() {
    if (this.isMemoryMode) {
      return;
    }
    try {
      const { Pool } = await import("pg");
      this.pool = new Pool({
        connectionString: this.config.connectionString,
      });
      await this._ensureSchema();
    } catch (e) {
      this.ctx.logger.warn(
        "Failed to connect to PostgreSQL, falling back to in-memory store:",
        (e as Error).message,
      );
      this.isMemoryMode = true;
    }
  }

  async _ensureSchema() {
    const client = await this.pool!.connect();
    try {
      await client.query("BEGIN");
      await client.query(`
        CREATE TABLE IF NOT EXISTS dashboards (
          id UUID PRIMARY KEY,
          version INT NOT NULL,
          spec JSONB NOT NULL,
          created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
          updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );
      `);
      await client.query(`
        CREATE TABLE IF NOT EXISTS dashboard_versions (
          id SERIAL PRIMARY KEY,
          dashboard_id UUID REFERENCES dashboards(id) ON DELETE CASCADE,
          version INT NOT NULL,
          spec JSONB NOT NULL,
          patch JSONB,
          actor VARCHAR,
          created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
          UNIQUE(dashboard_id, version)
        );
      `);
      await client.query(`
        CREATE TABLE IF NOT EXISTS plugin_state (
          id VARCHAR(255) PRIMARY KEY,
          enabled BOOLEAN NOT NULL,
          updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );
      `);
      await client.query(`
        CREATE TABLE IF NOT EXISTS audit_log (
          id SERIAL PRIMARY KEY,
          dashboard_id UUID,
          action VARCHAR NOT NULL,
          actor VARCHAR,
          patch JSONB,
          created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        );
      `);
      await client.query(`
        CREATE TABLE IF NOT EXISTS auth_sessions (
          id_hash VARCHAR(64) PRIMARY KEY,
          session_key VARCHAR(255) NOT NULL,
          sub VARCHAR(255),
          issuer VARCHAR,
          sid VARCHAR,
          email TEXT,
          name TEXT,
          preferred_username TEXT,
          groups JSONB NOT NULL DEFAULT '[]'::jsonb,
          scopes JSONB NOT NULL DEFAULT '[]'::jsonb,
          access_token TEXT,
          refresh_token TEXT,
          id_token TEXT,
          id_token_nonce TEXT,
          access_token_expires_at BIGINT,
          created_at BIGINT NOT NULL,
          last_seen_at BIGINT,
          expires_at BIGINT NOT NULL
        );
      `);
      await client.query(`
        CREATE INDEX IF NOT EXISTS auth_sessions_session_key_idx
          ON auth_sessions (session_key);
      `);
      await client.query(`
        CREATE INDEX IF NOT EXISTS auth_sessions_sub_idx
          ON auth_sessions (sub);
      `);
      await client.query(`
        CREATE INDEX IF NOT EXISTS auth_sessions_sid_idx
          ON auth_sessions (sid);
      `);
      await client.query(`
        CREATE TABLE IF NOT EXISTS service_tokens (
          id VARCHAR(64) PRIMARY KEY,
          token_hash VARCHAR(64) NOT NULL UNIQUE,
          name VARCHAR(255) NOT NULL,
          agent_id VARCHAR(255),
          skills JSONB NOT NULL DEFAULT '[]'::jsonb,
          scopes JSONB NOT NULL DEFAULT '[]'::jsonb,
          created_at BIGINT NOT NULL,
          last_used_at BIGINT,
          expires_at BIGINT,
          revoked_at BIGINT
        );
      `);
      await client.query("COMMIT");
    } catch (e) {
      await client.query("ROLLBACK");
      throw e;
    } finally {
      client.release();
    }
  }

  async getDashboard(id: string) {
    if (this.isMemoryMode) {
      const spec = this._memoryStore.get(id);
      if (!spec) throw new Error(`Dashboard not found: ${id}`);
      return spec;
    }
    const res = await this.pool!.query("SELECT spec FROM dashboards WHERE id = $1", [id]);
    if (res.rowCount === 0) {
      throw new Error(`Dashboard not found: ${id}`);
    }
    return res.rows[0].spec;
  }

  async listDashboards() {
    if (this.isMemoryMode) {
      return Array.from(this._memoryStore.values()).map((spec) => ({
        id: spec.id,
        title: spec.title || "Untitled",
        version: spec.version || 0,
      }));
    }
    const res = await this.pool!.query(
      "SELECT id, spec->>'title' as title, version FROM dashboards",
    );
    return res.rows;
  }

  async saveDashboard(spec: any, actor?: string, patch?: any) {
    if (this.isMemoryMode) {
      this._memoryStore.set(spec.id, spec);
      const versions = this._memoryVersions.get(spec.id) || [];
      versions.push({
        version: spec.version || 0,
        spec,
        patch: patch || null,
        actor: actor || "user",
        created_at: new Date().toISOString(),
      });
      this._memoryVersions.set(spec.id, versions);
      this._memoryAudit.push({
        dashboard_id: spec.id,
        action: "SAVE",
        actor: actor || "user",
        patch: patch || null,
        created_at: new Date().toISOString(),
      });
      return;
    }

    const client = await this.pool!.connect();
    try {
      await client.query("BEGIN");

      const id = spec.id;
      const version = spec.version || 1;

      // UPSERT dashboards
      await client.query(
        `
        INSERT INTO dashboards (id, version, spec, updated_at)
        VALUES ($1, $2, $3, CURRENT_TIMESTAMP)
        ON CONFLICT (id) DO UPDATE 
        SET version = EXCLUDED.version, spec = EXCLUDED.spec, updated_at = CURRENT_TIMESTAMP
      `,
        [id, version, spec],
      );

      // INSERT version
      await client.query(
        `
        INSERT INTO dashboard_versions (dashboard_id, version, spec, patch, actor)
        VALUES ($1, $2, $3, $4, $5)
      `,
        [id, version, spec, patch || null, actor || null],
      );

      // INSERT audit log
      await client.query(
        `
        INSERT INTO audit_log (dashboard_id, action, actor, patch)
        VALUES ($1, $2, $3, $4)
      `,
        [id, "SAVE", actor || null, patch || null],
      );

      await client.query("COMMIT");
    } catch (e) {
      await client.query("ROLLBACK");
      throw e;
    } finally {
      client.release();
    }
  }

  async getVersionHistory(dashboardId: string) {
    if (this.isMemoryMode) {
      return this._memoryVersions.get(dashboardId) || [];
    }
    const res = await this.pool!.query(
      "SELECT version, actor, created_at, patch FROM dashboard_versions WHERE dashboard_id = $1 ORDER BY version DESC",
      [dashboardId],
    );
    return res.rows;
  }

  async getVersion(dashboardId: string, version: number) {
    if (this.isMemoryMode) {
      const versions = this._memoryVersions.get(dashboardId) || [];
      const found = versions.find((v) => v.version === version);
      if (!found) throw new Error(`Version not found`);
      return found.spec;
    }
    const res = await this.pool!.query(
      "SELECT spec FROM dashboard_versions WHERE dashboard_id = $1 AND version = $2",
      [dashboardId, version],
    );
    if (res.rowCount === 0) {
      throw new Error(`Version not found`);
    }
    return res.rows[0].spec;
  }

  /**
   * Generic per-plugin enable/disable flag, used by the core plugin registry.
   *
   * `undefined` means "never set", which is what lets the registry tell a
   * plugin the user has never toggled (fall back to the manifest's
   * `defaultEnabled`) apart from one the user explicitly turned off. Collapsing
   * those two cases is how a toggle silently reverts on restart.
   */
  async getPluginState(id: string): Promise<boolean | undefined> {
    if (this.isMemoryMode) {
      return this._memoryPluginState.get(id);
    }
    const res = await this.pool!.query("SELECT enabled FROM plugin_state WHERE id = $1", [id]);
    if (res.rowCount === 0) return undefined;
    return res.rows[0].enabled === true || res.rows[0].enabled === "t";
  }

  async setPluginState(id: string, enabled: boolean): Promise<void> {
    if (this.isMemoryMode) {
      this._memoryPluginState.set(id, enabled);
      return;
    }
    await this.pool!.query(
      `
      INSERT INTO plugin_state (id, enabled, updated_at)
      VALUES ($1, $2, CURRENT_TIMESTAMP)
      ON CONFLICT (id) DO UPDATE SET enabled = EXCLUDED.enabled, updated_at = CURRENT_TIMESTAMP
    `,
      [id, enabled],
    );
  }

  async deleteDashboard(id: string) {
    if (this.isMemoryMode) {
      this._memoryStore.delete(id);
      this._memoryVersions.delete(id);
      return;
    }
    await this.pool!.query("DELETE FROM dashboards WHERE id = $1", [id]);
  }

  /* ---------------------------------------------------------------------- */
  /* Auth sessions                                                           */
  /*                                                                          */
  /* These back `ctx.auth`. Two deliberate choices:                           */
  /*                                                                          */
  /*  - The primary key is `id_hash` (sha256 of the cookie value), never the  */
  /*    cookie value itself. A dump of this table then yields nothing a      */
  /*    browser would accept, so a stolen backup is not a session.           */
  /*  - `rotateAuthSessionTokens` is a compare-and-swap on `refresh_token`.   */
  /*    Authentik's `refresh_token_threshold` defaults to 0, i.e. the         */
  /*    refresh token is ALWAYS renewed, so the returned string differs from  */
  /*    the one sent and the old one is dead. Two concurrent requests that   */
  /*    both read the same token would therefore see one succeed and one      */
  /*    `invalid_grant`. The CAS makes exactly one of them win.              */
  /* ---------------------------------------------------------------------- */

  async createAuthSession(record: any): Promise<void> {
    if (this.isMemoryMode) {
      this._memoryAuthSessions.set(record.idHash, { ...record });
      return;
    }
    await this.pool!.query(
      `
      INSERT INTO auth_sessions (
        id_hash, session_key, sub, issuer, sid, email, name, preferred_username,
        groups, scopes, access_token, refresh_token, id_token, id_token_nonce,
        access_token_expires_at, created_at, last_seen_at, expires_at
      ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18)
    `,
      [
        record.idHash,
        record.sessionKey,
        record.sub ?? null,
        record.issuer ?? null,
        record.sid ?? null,
        record.email ?? null,
        record.name ?? null,
        record.preferredUsername ?? null,
        JSON.stringify(record.groups ?? []),
        JSON.stringify(record.scopes ?? []),
        record.accessToken ?? null,
        record.refreshToken ?? null,
        record.idToken ?? null,
        record.idTokenNonce ?? null,
        record.accessTokenExpiresAt ?? null,
        record.createdAt,
        record.lastSeenAt ?? null,
        record.expiresAt,
      ],
    );
  }

  async getAuthSession(idHash: string): Promise<any | undefined> {
    if (this.isMemoryMode) {
      const row = this._memoryAuthSessions.get(idHash);
      return row ? { ...row } : undefined;
    }
    const res = await this.pool!.query("SELECT * FROM auth_sessions WHERE id_hash = $1", [idHash]);
    if (res.rowCount === 0) return undefined;
    return normalizeAuthSessionRow(res.rows[0]);
  }

  /**
   * Atomically replace a session's token set, but ONLY if `refresh_token` is
   * still the value the caller read.
   *
   * Returns false when another request rotated first. The caller must then
   * discard its new tokens and use the session as it now stands, rather than
   * persisting a refresh token that is already dead.
   */
  async rotateAuthSessionTokens(
    idHash: string,
    expectedRefreshToken: string | null,
    next: {
      accessToken?: string | null;
      refreshToken?: string | null;
      idToken?: string | null;
      accessTokenExpiresAt?: number | null;
    },
  ): Promise<boolean> {
    if (this.isMemoryMode) {
      const row = this._memoryAuthSessions.get(idHash);
      if (!row) return false;
      if ((row.refreshToken ?? null) !== (expectedRefreshToken ?? null)) return false;
      if (next.accessToken !== undefined) row.accessToken = next.accessToken;
      if (next.refreshToken !== undefined) row.refreshToken = next.refreshToken;
      if (next.idToken !== undefined) row.idToken = next.idToken;
      if (next.accessTokenExpiresAt !== undefined) {
        row.accessTokenExpiresAt = next.accessTokenExpiresAt;
      }
      row.lastSeenAt = Date.now();
      return true;
    }
    const res = await this.pool!.query(
      `
      UPDATE auth_sessions SET
        access_token = COALESCE($2, access_token),
        refresh_token = COALESCE($3, refresh_token),
        id_token = COALESCE($4, id_token),
        access_token_expires_at = COALESCE($5, access_token_expires_at),
        last_seen_at = CURRENT_TIMESTAMP * 1000
      WHERE id_hash = $1 AND refresh_token IS NOT DISTINCT FROM $6
    `,
      [
        idHash,
        next.accessToken ?? null,
        next.refreshToken ?? null,
        next.idToken ?? null,
        next.accessTokenExpiresAt ?? null,
        expectedRefreshToken ?? null,
      ],
    );
    return (res.rowCount ?? 0) > 0;
  }

  async touchAuthSession(idHash: string, lastSeenAt: number): Promise<void> {
    if (this.isMemoryMode) {
      const row = this._memoryAuthSessions.get(idHash);
      if (row) this._memoryAuthSessions.set(idHash, { ...row, lastSeenAt });
      return;
    }
    await this.pool!.query("UPDATE auth_sessions SET last_seen_at = $2 WHERE id_hash = $1", [
      idHash,
      lastSeenAt,
    ]);
  }

  async deleteAuthSession(idHash: string): Promise<void> {
    if (this.isMemoryMode) {
      this._memoryAuthSessions.delete(idHash);
      return;
    }
    await this.pool!.query("DELETE FROM auth_sessions WHERE id_hash = $1", [idHash]);
  }

  /** Every session belonging to one subject. Used by logout and by revocation. */
  async listAuthSessionsBySessionKey(sessionKey: string): Promise<any[]> {
    if (this.isMemoryMode) {
      return [...this._memoryAuthSessions.values()]
        .filter((row) => row.sessionKey === sessionKey)
        .map((row) => ({ ...row }));
    }
    const res = await this.pool!.query("SELECT * FROM auth_sessions WHERE session_key = $1", [
      sessionKey,
    ]);
    return res.rows.map(normalizeAuthSessionRow);
  }

  /** Sessions for a raw `sub`. Backs back-channel logout when no `sid` is present. */
  async listAuthSessionsBySub(sub: string): Promise<any[]> {
    if (this.isMemoryMode) {
      return [...this._memoryAuthSessions.values()]
        .filter((row) => row.sub === sub)
        .map((row) => ({ ...row }));
    }
    const res = await this.pool!.query("SELECT * FROM auth_sessions WHERE sub = $1", [sub]);
    return res.rows.map(normalizeAuthSessionRow);
  }

  /**
   * Every session matching an OIDC `sid`.
   *
   * Back-channel logout is keyed on `sid`, not on `sub`: one subject signed in
   * to two browsers has two `sid`s, and a logout event only names the one that
   * ended. Destroying by `session_key` instead would sign the user out of every
   * device, which is a different (and surprising) behaviour.
   */
  async listAuthSessionsBySid(sid: string): Promise<any[]> {
    if (this.isMemoryMode) {
      return [...this._memoryAuthSessions.values()]
        .filter((row) => row.sid === sid)
        .map((row) => ({ ...row }));
    }
    const res = await this.pool!.query("SELECT * FROM auth_sessions WHERE sid = $1", [sid]);
    return res.rows.map(normalizeAuthSessionRow);
  }

  /** Drop expired sessions. Returns how many went. */
  async pruneAuthSessions(now: number): Promise<number> {
    if (this.isMemoryMode) {
      let removed = 0;
      for (const [key, row] of [...this._memoryAuthSessions]) {
        if ((row.expiresAt ?? 0) <= now) {
          this._memoryAuthSessions.delete(key);
          removed += 1;
        }
      }
      return removed;
    }
    const res = await this.pool!.query("DELETE FROM auth_sessions WHERE expires_at <= $1", [now]);
    return res.rowCount ?? 0;
  }

  /* ---------------------------------------------------------------------- */
  /* Service tokens                                                          */
  /*                                                                          */
  /* Hashed at rest for the same reason sessions are: the plaintext exists     */
  /* exactly once, in the response to the create call.                        */
  /* ---------------------------------------------------------------------- */

  async createServiceToken(record: any): Promise<void> {
    if (this.isMemoryMode) {
      this._memoryServiceTokens.set(record.id, { ...record });
      return;
    }
    await this.pool!.query(
      `
      INSERT INTO service_tokens (id, token_hash, name, agent_id, skills, scopes, created_at, expires_at)
      VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
    `,
      [
        record.id,
        record.tokenHash,
        record.name,
        record.agentId ?? null,
        JSON.stringify(record.skills ?? []),
        JSON.stringify(record.scopes ?? []),
        record.createdAt,
        record.expiresAt ?? null,
      ],
    );
  }

  async getServiceTokenByHash(tokenHash: string): Promise<any | undefined> {
    if (this.isMemoryMode) {
      const row = this._memoryServiceTokens.get(tokenHash);
      return row ? { ...row } : undefined;
    }
    const res = await this.pool!.query("SELECT * FROM service_tokens WHERE token_hash = $1", [
      tokenHash,
    ]);
    if (res.rowCount === 0) return undefined;
    return normalizeServiceTokenRow(res.rows[0]);
  }

  async touchServiceToken(id: string, lastUsedAt: number): Promise<void> {
    if (this.isMemoryMode) {
      const row = this._memoryServiceTokens.get(id);
      if (row) this._memoryServiceTokens.set(id, { ...row, lastUsedAt });
      return;
    }
    await this.pool!.query("UPDATE service_tokens SET last_used_at = $2 WHERE id = $1", [
      id,
      lastUsedAt,
    ]);
  }

  async revokeServiceToken(id: string): Promise<void> {
    if (this.isMemoryMode) {
      const row = this._memoryServiceTokens.get(id);
      if (row) this._memoryServiceTokens.set(id, { ...row, revokedAt: Date.now() });
      return;
    }
    await this.pool!.query("UPDATE service_tokens SET revoked_at = $2 WHERE id = $1", [
      id,
      Date.now(),
    ]);
  }

  async listServiceTokens(): Promise<any[]> {
    if (this.isMemoryMode) {
      return [...this._memoryServiceTokens.values()].map((row) => ({ ...row }));
    }
    const res = await this.pool!.query("SELECT * FROM service_tokens ORDER BY created_at DESC");
    return res.rows.map(normalizeServiceTokenRow);
  }
}

/** Postgres returns JSONB as a parsed value and BIGINT as a string. Normalize both. */
function normalizeAuthSessionRow(row: any): any {
  return {
    idHash: row.id_hash,
    sessionKey: row.session_key,
    sub: row.sub ?? row.session_key,
    issuer: row.issuer ?? undefined,
    sid: row.sid ?? undefined,
    email: row.email ?? undefined,
    name: row.name ?? undefined,
    preferredUsername: row.preferred_username ?? undefined,
    groups: toStringArray(row.groups),
    scopes: toStringArray(row.scopes),
    accessToken: row.access_token ?? undefined,
    refreshToken: row.refresh_token ?? undefined,
    idToken: row.id_token ?? undefined,
    idTokenNonce: row.id_token_nonce ?? undefined,
    accessTokenExpiresAt: toNumberOrUndefined(row.access_token_expires_at),
    createdAt: toNumberOrUndefined(row.created_at) ?? 0,
    lastSeenAt: toNumberOrUndefined(row.last_seen_at),
    expiresAt: toNumberOrUndefined(row.expires_at) ?? 0,
  };
}

function normalizeServiceTokenRow(row: any): any {
  return {
    id: row.id,
    tokenHash: row.token_hash,
    name: row.name,
    agentId: row.agent_id ?? undefined,
    skills: toStringArray(row.skills),
    scopes: toStringArray(row.scopes),
    createdAt: toNumberOrUndefined(row.created_at) ?? 0,
    lastUsedAt: toNumberOrUndefined(row.last_used_at),
    expiresAt: toNumberOrUndefined(row.expires_at),
    revokedAt: toNumberOrUndefined(row.revoked_at),
  };
}

function toStringArray(value: unknown): string[] {
  if (Array.isArray(value)) return value.map((entry) => String(entry));
  if (typeof value === "string") {
    try {
      const parsed = JSON.parse(value);
      if (Array.isArray(parsed)) return parsed.map((entry) => String(entry));
    } catch {
      /* fall through */
    }
  }
  return [];
}

function toNumberOrUndefined(value: unknown): number | undefined {
  if (value === null || value === undefined) return undefined;
  const parsed = typeof value === "number" ? value : Number(value);
  return Number.isFinite(parsed) ? parsed : undefined;
}

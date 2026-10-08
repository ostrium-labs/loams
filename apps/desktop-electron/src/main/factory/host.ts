import { Context } from "cordis";
import type {
	FactoryAppId,
	FactoryAppInfo,
	FactoryHealth,
	FactoryQuery,
	IpcResult,
} from "../../shared/contracts";
import { redact as redactSecrets } from "../redact";
import { type Adapter, FACTORY_APPS, type FactoryAppDef } from "./apps";
import { OPS, ParamError, UnsupportedError } from "./ops";
import type { Vault } from "./vault";

export const QUERY_TIMEOUT_MS = 15_000;

const b64 = (s: string): string[] => {
	const full = Buffer.from(s).toString("base64");
	return [full, full.replace(/=+$/, "")];
};

/** Every form a credential can take in a message: raw, URL-encoded, form-encoded, Basic. */
export function secretForms(
	secrets: readonly string[],
	plain: readonly string[] = [],
): string[] {
	const forms = new Set<string>();
	for (const s of secrets.filter((x) => x.length > 0)) {
		forms.add(s);
		forms.add(encodeURIComponent(s));
		forms.add(new URLSearchParams({ k: s }).toString().slice(2));
		for (const f of b64(s)) forms.add(f);
		// HTTP Basic: base64(user:pass) with any configured field as the user.
		for (const u of [...plain, ...secrets]) {
			if (!u || u === s) continue;
			for (const f of [...b64(`${u}:${s}`), ...b64(`${s}:${u}`)]) forms.add(f);
		}
	}
	return [...forms];
}

/** Factory errors and agent text: every secret form becomes `[redacted]` (see ../redact). */
export function redact(message: string, secrets: readonly string[]): string {
	return redactSecrets(message, secrets);
}

const originOf = (u: string): string => {
	try {
		return new URL(u).origin;
	} catch {
		return "";
	}
};

type Code = "auth_failed" | "unreachable" | "upstream_error";

export function classifyError(e: unknown): Code {
	const err = (typeof e === "object" && e !== null ? e : {}) as {
		status?: unknown;
		name?: unknown;
		code?: unknown;
		message?: unknown;
	};
	const status = typeof err.status === "number" ? err.status : undefined;
	if (status === 401 || status === 403) return "auth_failed";
	if (status === 0) return "unreachable";
	const msg = typeof err.message === "string" ? err.message : "";
	if (
		status !== undefined &&
		!(err.name === "MatomoApiError" && status === 200)
	)
		return "upstream_error";
	// Matomo reports a bad token as an error inside a 200 body.
	if (
		err.name === "MatomoApiError" &&
		/token|auth|permission|access|credential/i.test(msg)
	)
		return "auth_failed";
	if (/invalid api key|unauthorized|forbidden/i.test(msg)) return "auth_failed";
	if (
		err.name === "TypeError" ||
		err.name === "AbortError" ||
		err.name === "TimeoutError" ||
		(typeof err.code === "string" &&
			/^E(CONN|NOTFOUND|HOSTUNREACH|TIMEDOUT)/.test(err.code))
	)
		return "unreachable";
	return "upstream_error";
}

interface Live {
	adapter: Adapter;
	/** Every form of the secrets this adapter was built with (survives reconfigure). */
	secrets: string[];
	dispose(): void;
}

type CredField = FactoryAppInfo["credentialFields"][number];

export class FactoryHost {
	readonly #live = new Map<FactoryAppId, Live>();
	readonly #building = new Map<FactoryAppId, Promise<Live>>();
	readonly #gen = new Map<FactoryAppId, number>();
	readonly #health = new Map<FactoryAppId, FactoryHealth>();

	constructor(
		private readonly vault: Vault,
		private readonly ctx: Context = new Context(),
		private readonly apps: Record<FactoryAppId, FactoryAppDef> = FACTORY_APPS,
	) {}

	#info(app: FactoryAppId): FactoryAppInfo {
		const def = this.apps[app];
		const entry = this.vault.get(app);
		return {
			id: app,
			label: def.label,
			...(entry ? { url: entry.url } : {}),
			health: entry
				? (this.#health.get(app) ?? "unconfigured")
				: this.vault.isLocked(app)
					? "locked"
					: "unconfigured",
			hasPanels: def.hasPanels,
			credentialFields: def.credentialFields.map((f: CredField) => ({ ...f })),
			...(entry ? { fields: this.#plainFields(app) } : {}),
			persistent: this.vault.persistent,
		};
	}

	async list(): Promise<FactoryAppInfo[]> {
		return (Object.keys(this.apps) as FactoryAppId[]).map((a) => this.#info(a));
	}

	/** The configured URL and SSO origin of an app (used by the factory view). */
	appUrls(app: FactoryAppId): { url: string; ssoOrigin?: string } | undefined {
		const e = this.vault.get(app);
		if (!e) return undefined;
		const sso = e.fields["ssoOrigin"]?.reveal();
		return { url: e.url, ...(sso ? { ssoOrigin: sso } : {}) };
	}

	#secrets(app: FactoryAppId): string[] {
		const e = this.vault.get(app);
		if (!e) return [];
		const secret = this.apps[app].credentialFields
			.filter((f) => f.secret)
			.map((f) => e.fields[f.key]?.reveal() ?? "")
			.filter(Boolean);
		const plain = Object.values(this.#plainFields(app));
		return secretForms(secret, plain);
	}

	/** Every configured app's secrets in every encoded form (the agent transcript scrubber). */
	allSecrets(): string[] {
		const out = new Set<string>();
		for (const app of Object.keys(this.apps) as FactoryAppId[])
			for (const f of this.#secrets(app)) out.add(f);
		return [...out];
	}

	#plainFields(app: FactoryAppId): Record<string, string> {
		const e = this.vault.get(app);
		const out: Record<string, string> = {};
		if (!e) return out;
		for (const f of this.apps[app].credentialFields)
			if (!f.secret && e.fields[f.key])
				out[f.key] = e.fields[f.key]?.reveal() ?? "";
		return out;
	}

	#fail(secrets: string[], e: unknown): { code: string; message: string } {
		const raw = e instanceof Error ? e.message : String(e);
		const code =
			e instanceof ParamError
				? "bad_params"
				: e instanceof UnsupportedError
					? "unsupported"
					: classifyError(e);
		return { code, message: redact(raw, secrets) };
	}

	#dispose(app: FactoryAppId): void {
		this.#gen.set(app, (this.#gen.get(app) ?? 0) + 1);
		const building = this.#building.get(app);
		this.#building.delete(app);
		const closeLive = (live: Live) => {
			try {
				(live.adapter as { detach?: () => void }).detach?.();
			} catch {
				// best effort
			}
			live.dispose();
		};
		const live = this.#live.get(app);
		this.#live.delete(app);
		if (live) closeLive(live);
		// A build still in flight is discarded by its generation check.
		void building?.then(
			(l) => {
				if (this.#live.get(app) !== l) closeLive(l);
			},
			() => undefined,
		);
	}

	/** Single-flight: concurrent callers share one build; a stale build is discarded. */
	#adapter(app: FactoryAppId): Promise<Live> {
		const existing = this.#live.get(app);
		if (existing) return Promise.resolve(existing);
		const inflight = this.#building.get(app);
		if (inflight) return inflight;
		const gen = this.#gen.get(app) ?? 0;
		const p = this.#build(app).then(async (live) => {
			if ((this.#gen.get(app) ?? 0) !== gen) {
				try {
					(live.adapter as { detach?: () => void }).detach?.();
				} catch {
					// best effort
				}
				live.dispose();
				throw new Error("configuration changed while connecting");
			}
			this.#live.set(app, live);
			this.#building.delete(app);
			return live;
		});
		this.#building.set(app, p);
		p.catch(() => {
			if (this.#building.get(app) === p) this.#building.delete(app);
		});
		return p;
	}

	async #build(app: FactoryAppId): Promise<Live> {
		const def = this.apps[app];
		const entry = this.vault.get(app);
		if (!entry || !def.adapter) throw new Error("not configured");
		const secrets = this.#secrets(app);
		const Ctor = await def.adapter();
		const raw: Record<string, string> = {};
		for (const [k, s] of Object.entries(entry.fields)) raw[k] = s.reveal();
		const config = def.configFrom(entry.url, raw);
		let adapter: Adapter | undefined;
		// One child scope per app: disposing it tears the adapter service down.
		const scope = this.ctx.plugin((child: Context) => {
			adapter = new Ctor(child, config);
		});
		try {
			await scope;
		} catch (e) {
			scope.dispose();
			throw e;
		}
		if (!adapter) throw new Error("adapter failed to start");
		return { adapter, secrets, dispose: () => scope.dispose() };
	}

	async configure(
		app: FactoryAppId,
		url: string,
		fields: Record<string, string>,
	): Promise<IpcResult<FactoryAppInfo>> {
		const def = this.apps[app] as FactoryAppDef | undefined;
		if (!def) return { ok: false, code: "unknown_app", message: "Unknown app" };
		try {
			const u = new URL(url);
			if (u.protocol !== "http:" && u.protocol !== "https:") throw new Error();
			if (u.username || u.password)
				return {
					ok: false,
					code: "invalid_url",
					message: "Put credentials in the credential fields, not in the URL",
				};
		} catch {
			return {
				ok: false,
				code: "invalid_url",
				message: "Enter an http(s) URL",
			};
		}
		const allowed = new Set(def.credentialFields.map((f) => f.key));
		const clean: Record<string, string> = {};
		for (const [k, v] of Object.entries(fields))
			if (allowed.has(k) && typeof v === "string" && v.length > 0) clean[k] = v;
		// Reconfiguring keeps a stored value for any field left blank, except that a
		// secret is never carried to a different origin: it must be entered again.
		const stored = this.vault.get(app);
		const sameOrigin =
			stored !== undefined && originOf(stored.url) === originOf(url);
		if (stored)
			for (const f of def.credentialFields) {
				if (f.secret && !sameOrigin) continue;
				const kept = stored.fields[f.key]?.reveal();
				if (!clean[f.key] && kept) clean[f.key] = kept;
			}
		const missing = def.credentialFields.find((f) => f.secret && !clean[f.key]);
		if (missing)
			return {
				ok: false,
				code: "bad_fields",
				message:
					stored && !sameOrigin
						? `${missing.label} is required again: the URL points to a different origin`
						: `${missing.label} is required`,
			};
		this.#dispose(app);
		this.vault.set(app, url, clean);
		this.#health.delete(app);
		const info = await this.test(app);
		return { ok: true, value: info };
	}

	async test(app: FactoryAppId): Promise<FactoryAppInfo> {
		const def = this.apps[app];
		if (!this.vault.get(app)) {
			this.#health.delete(app);
			return this.#info(app);
		}
		if (!def.adapter) {
			// No adapter (OpenObserve): configured means nothing to probe.
			this.#health.set(app, "ok");
			return this.#info(app);
		}
		const gen = this.#gen.get(app) ?? 0;
		let health: FactoryHealth = "ok";
		try {
			const live = await this.#adapter(app);
			await this.#timed(
				OPS[app]["health"]?.run(live.adapter, undefined as never, {
					fields: this.#plainFields(app),
				}),
			);
		} catch (e) {
			health =
				classifyError(e) === "auth_failed" ? "auth_failed" : "unreachable";
		}
		// A newer configure/remove superseded this probe: do not write its result.
		if ((this.#gen.get(app) ?? 0) === gen) this.#health.set(app, health);
		return this.#info(app);
	}

	async remove(app: FactoryAppId): Promise<void> {
		this.#dispose(app);
		this.vault.remove(app);
		this.#health.delete(app);
	}

	#timed<T>(p: Promise<T> | undefined): Promise<T> {
		return new Promise<T>((resolve, reject) => {
			const t = setTimeout(
				() =>
					reject(
						Object.assign(new Error("request timed out"), {
							name: "TimeoutError",
						}),
					),
				QUERY_TIMEOUT_MS,
			);
			(p ?? Promise.reject(new Error("unsupported"))).then(
				(v) => {
					clearTimeout(t);
					resolve(v);
				},
				(e) => {
					clearTimeout(t);
					reject(e);
				},
			);
		});
	}

	async query(q: FactoryQuery): Promise<IpcResult<unknown>> {
		const ops = Object.hasOwn(OPS, q?.app ?? "") ? OPS[q.app] : undefined;
		if (!ops) return { ok: false, code: "unknown_app", message: "Unknown app" };
		const op =
			typeof q.op === "string" && Object.hasOwn(ops, q.op)
				? ops[q.op]
				: undefined;
		if (!op)
			return {
				ok: false,
				code: "unknown_op",
				message: `Unknown operation: ${String(q.op).slice(0, 40)}`,
			};
		const parsed = op.params.safeParse(q.params ?? {});
		if (!parsed.success)
			return {
				ok: false,
				code: "bad_params",
				message: parsed.error.issues
					.map((i) => `${i.path.join(".") || "params"}: ${i.message}`)
					.join("; "),
			};
		if (!this.vault.get(q.app))
			return {
				ok: false,
				code: "unconfigured",
				message: "This app is not configured yet",
			};
		// Snapshot before any await: a reconfigure mid-flight must not change
		// which secrets this query's error message is scrubbed of.
		const secrets = this.#secrets(q.app);
		const fields = this.#plainFields(q.app);
		try {
			const live = await this.#adapter(q.app);
			secrets.push(...live.secrets);
			const value = await this.#timed(
				op.run(live.adapter, parsed.data as never, { fields }),
			);
			return { ok: true, value };
		} catch (e) {
			return { ok: false, ...this.#fail(secrets, e) };
		}
	}
}

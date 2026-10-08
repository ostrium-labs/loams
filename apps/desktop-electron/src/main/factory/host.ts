import { Context } from "cordis";
import type {
	FactoryAppId,
	FactoryAppInfo,
	FactoryHealth,
	FactoryQuery,
	IpcResult,
} from "../../shared/contracts";
import { type Adapter, FACTORY_APPS, type FactoryAppDef } from "./apps";
import { OPS, ParamError } from "./ops";
import type { Vault } from "./vault";

export const QUERY_TIMEOUT_MS = 15_000;

export function redact(message: string, secrets: readonly string[]): string {
	let out = message;
	const all = secrets
		.filter((s) => s.length > 0)
		.flatMap((s) => [s, encodeURIComponent(s)])
		.sort((a, b) => b.length - a.length);
	for (const s of all) out = out.split(s).join("[redacted]");
	return out;
}

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
	if (status !== undefined) return "upstream_error";
	const msg = typeof err.message === "string" ? err.message : "";
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
	dispose(): void;
}

type CredField = FactoryAppInfo["credentialFields"][number];

export class FactoryHost {
	readonly #live = new Map<FactoryAppId, Live>();
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
				: "unconfigured",
			hasPanels: def.hasPanels,
			credentialFields: def.credentialFields.map((f: CredField) => ({ ...f })),
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
		return this.apps[app].credentialFields
			.filter((f) => f.secret)
			.map((f) => e.fields[f.key]?.reveal() ?? "")
			.filter(Boolean);
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

	#fail(app: FactoryAppId, e: unknown): { code: string; message: string } {
		const raw = e instanceof Error ? e.message : String(e);
		const code = e instanceof ParamError ? "bad_params" : classifyError(e);
		return { code, message: redact(raw, this.#secrets(app)) };
	}

	#dispose(app: FactoryAppId): void {
		const live = this.#live.get(app);
		this.#live.delete(app);
		if (!live) return;
		try {
			(live.adapter as { detach?: () => void }).detach?.();
		} catch {
			// best effort
		}
		live.dispose();
	}

	async #adapter(app: FactoryAppId): Promise<Adapter> {
		const existing = this.#live.get(app);
		if (existing) return existing.adapter;
		const def = this.apps[app];
		const entry = this.vault.get(app);
		if (!entry || !def.adapter) throw new Error("not configured");
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
		this.#live.set(app, { adapter, dispose: () => scope.dispose() });
		return adapter;
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
		} catch {
			return { ok: false, code: "bad_url", message: "Enter an http(s) URL" };
		}
		const allowed = new Set(def.credentialFields.map((f) => f.key));
		const clean: Record<string, string> = {};
		for (const [k, v] of Object.entries(fields))
			if (allowed.has(k) && typeof v === "string" && v.length > 0) clean[k] = v;
		const missing = def.credentialFields.find((f) => f.secret && !clean[f.key]);
		if (missing)
			return {
				ok: false,
				code: "bad_fields",
				message: `${missing.label} is required`,
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
		try {
			const adapter = await this.#adapter(app);
			await this.#timed(
				OPS[app]["health"]?.run(adapter, undefined as never, {
					fields: this.#plainFields(app),
				}),
			);
			this.#health.set(app, "ok");
		} catch (e) {
			this.#health.set(
				app,
				classifyError(e) === "auth_failed" ? "auth_failed" : "unreachable",
			);
		}
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
		try {
			const adapter = await this.#adapter(q.app);
			const value = await this.#timed(
				op.run(adapter, parsed.data as never, {
					fields: this.#plainFields(q.app),
				}),
			);
			return { ok: true, value };
		} catch (e) {
			return { ok: false, ...this.#fail(q.app, e) };
		}
	}
}

// The factory as Electron main sees it: a client of loams-agentd's FactoryService
// (SF1 amendment F1-F4; design §50 §13, D795). The daemon holds the apps'
// credentials in its keyring and runs the read-only op allowlist; main passes a
// configure form's secrets through once and never stores or returns them. Main
// keeps the embedded views (D678), so it caches the apps' non-secret URLs.
import { type Client, createClient, type Transport } from "@connectrpc/connect";
import {
	type FactoryApp,
	FactoryService,
	FactoryHealth as PbHealth,
} from "@loams/proto/agentd";
import type {
	FactoryAppId,
	FactoryAppInfo,
	FactoryHealth,
	FactoryQuery,
	IpcResult,
} from "../../shared/contracts";
import { redact as redactSecrets } from "../redact";
import { FACTORY_APP_IDS, isFactoryApp } from "./apps";
import type { Vault } from "./vault";

/** The longest a factory call may take (the daemon's own limit is 15 s). */
export const QUERY_TIMEOUT_MS = 20_000;

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

/** Agent text: every secret form becomes `[redacted]` (see ../redact). */
export function redact(message: string, secrets: readonly string[]): string {
	return redactSecrets(message, secrets);
}

/** What the factory pages show when the daemon does not answer. */
export const DAEMON_DOWN =
	"The factory's apps are reached through the agent daemon, which is not running. Start it from Studio.";

const HEALTH: Record<number, FactoryHealth> = {
	[PbHealth.OK]: "ok",
	[PbHealth.AUTH_FAILED]: "auth_failed",
	[PbHealth.UNREACHABLE]: "unreachable",
	[PbHealth.UNCONFIGURED]: "unconfigured",
	[PbHealth.LOCKED]: "locked",
};

/** A daemon app as the console's `FactoryAppInfo`. */
export function appInfo(a: FactoryApp): FactoryAppInfo {
	const configured = a.url !== undefined;
	return {
		id: a.id as FactoryAppId,
		label: a.label,
		...(configured ? { url: a.url } : {}),
		health: HEALTH[a.health] ?? "unreachable",
		hasPanels: a.hasPanels,
		...(a.preview ? { preview: a.preview } : {}),
		credentialFields: a.credentialFields.map((f) => ({
			key: f.key,
			label: f.label,
			secret: f.secret,
		})),
		...(configured ? { fields: { ...a.fields } } : {}),
		persistent: a.persistent,
	};
}

const fail = (code: string, message: string) =>
	({ ok: false, code, message }) as const;

export class FactoryHost {
	readonly #apps = new Map<FactoryAppId, FactoryAppInfo>();

	/** `transport` answers null while no daemon runs. */
	constructor(private readonly transport: () => Transport | null) {}

	#client(): Client<typeof FactoryService> | null {
		const t = this.transport();
		return t ? createClient(FactoryService, t) : null;
	}

	#remember(info: FactoryAppInfo): FactoryAppInfo {
		this.#apps.set(info.id, info);
		return info;
	}

	/** Every app; throws `DAEMON_DOWN` when the daemon does not answer. */
	async list(): Promise<FactoryAppInfo[]> {
		const c = this.#client();
		if (!c) throw new Error(DAEMON_DOWN);
		let r: Awaited<ReturnType<typeof c.listFactoryApps>>;
		try {
			r = await c.listFactoryApps({}, { timeoutMs: QUERY_TIMEOUT_MS });
		} catch {
			throw new Error(DAEMON_DOWN);
		}
		this.#apps.clear();
		return r.apps.map((a) => this.#remember(appInfo(a)));
	}

	/**
	 * The app's own UI (its App URL where set, as Plane's) and its SSO origin, for
	 * the embedded views; from the last list, configure or test.
	 */
	appUrls(app: FactoryAppId): { url: string; ssoOrigin?: string } | undefined {
		const info = this.#apps.get(app);
		if (!info?.url) return undefined;
		const ui = info.fields?.["appUrl"];
		const sso = info.fields?.["ssoOrigin"];
		return { url: ui || info.url, ...(sso ? { ssoOrigin: sso } : {}) };
	}

	/** Main never holds a factory secret: the daemon scrubs its own errors. */
	allSecrets(): string[] {
		return [];
	}

	async configure(
		app: FactoryAppId,
		url: string,
		fields: Record<string, string>,
	): Promise<IpcResult<FactoryAppInfo>> {
		const c = this.#client();
		if (!c) return fail("daemon_down", DAEMON_DOWN);
		try {
			const r = await c.configureFactoryApp(
				{ app, url, fields },
				{ timeoutMs: QUERY_TIMEOUT_MS },
			);
			if (r.result.case === "app")
				return { ok: true, value: this.#remember(appInfo(r.result.value)) };
			if (r.result.case === "error")
				return fail(r.result.value.code, r.result.value.message);
			return fail("internal", "The daemon gave no answer");
		} catch {
			return fail("daemon_down", DAEMON_DOWN);
		}
	}

	async test(app: FactoryAppId): Promise<IpcResult<FactoryAppInfo>> {
		const c = this.#client();
		if (!c) return fail("daemon_down", DAEMON_DOWN);
		try {
			const r = await c.testFactoryApp(
				{ app },
				{ timeoutMs: QUERY_TIMEOUT_MS },
			);
			if (r.result.case === "app")
				return { ok: true, value: this.#remember(appInfo(r.result.value)) };
			if (r.result.case === "error")
				return fail(r.result.value.code, r.result.value.message);
			return fail("internal", "The daemon gave no answer");
		} catch {
			return fail("daemon_down", DAEMON_DOWN);
		}
	}

	async remove(app: FactoryAppId): Promise<void> {
		this.#apps.delete(app);
		const c = this.#client();
		if (!c) throw new Error(DAEMON_DOWN);
		await c.removeFactoryApp({ app }, { timeoutMs: QUERY_TIMEOUT_MS });
	}

	async query(q: FactoryQuery): Promise<IpcResult<unknown>> {
		if (!isFactoryApp(q?.app)) return fail("unknown_app", "Unknown app");
		const c = this.#client();
		if (!c) return fail("daemon_down", DAEMON_DOWN);
		let paramsJson: string;
		try {
			paramsJson = JSON.stringify(q.params ?? {});
		} catch {
			return fail("bad_params", "params: not JSON");
		}
		try {
			const r = await c.queryFactory(
				{ app: q.app, op: String(q.op), paramsJson },
				{ timeoutMs: QUERY_TIMEOUT_MS },
			);
			if (r.result.case === "valueJson")
				return { ok: true, value: JSON.parse(r.result.value) };
			if (r.result.case === "error")
				return fail(r.result.value.code, r.result.value.message);
			return fail("internal", "The daemon gave no answer");
		} catch {
			return fail("daemon_down", DAEMON_DOWN);
		}
	}

	/**
	 * Move the factory credentials the TypeScript vault kept before 0.2 into the
	 * daemon (§50 §12, "Migration from Electron"). An entry leaves the vault only
	 * once the daemon has stored it; a failure keeps it for the next attempt.
	 */
	async importLegacy(vault: Vault): Promise<number> {
		let moved = 0;
		for (const app of FACTORY_APP_IDS) {
			const entry = vault.get(app);
			if (!entry) continue;
			const fields: Record<string, string> = {};
			for (const [k, s] of Object.entries(entry.fields)) fields[k] = s.reveal();
			const r = await this.configure(app, entry.url, fields);
			if (r.ok) {
				vault.remove(app);
				moved++;
			}
		}
		return moved;
	}
}

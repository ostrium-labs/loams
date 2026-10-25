import type { FactoryAppId, FactoryAppInfo } from "../../shared/contracts";

// biome-ignore lint/suspicious/noExplicitAny: adapters are a heterogeneous set of cordis services; ops.ts narrows per app.
export type Adapter = Record<string, (...args: any[]) => Promise<any>>;
// biome-ignore lint/suspicious/noExplicitAny: per-adapter config shapes differ.
export type AdapterCtor = new (ctx: any, config: any) => Adapter;
export type CredentialField = FactoryAppInfo["credentialFields"][number];

export interface FactoryAppDef {
	label: string;
	credentialFields: CredentialField[];
	hasPanels: boolean;
	/** The honest status label while the app is a preview (shown in the UI). */
	preview?: string;
	/** Absent for SigNoz: full UI only. */
	adapter?: () => Promise<AdapterCtor>;
	/**
	 * Health for an app without an adapter: resolves when the app answers, throws
	 * (with `status` for an HTTP error) when it does not.
	 */
	probe?: (url: string) => Promise<void>;
	configFrom(
		url: string,
		fields: Record<string, string>,
	): Record<string, unknown>;
}

const sso: CredentialField = {
	key: "ssoOrigin",
	label: "SSO origin (optional)",
	secret: false,
};
const f = (key: string, label: string, secret = true): CredentialField => ({
	key,
	label,
	secret,
});

const base = (url: string) => url.replace(/\/+$/, "");

export const FACTORY_APPS: Record<FactoryAppId, FactoryAppDef> = {
	forgejo: {
		label: "Forgejo",
		credentialFields: [f("token", "Access token"), sso],
		hasPanels: true,
		adapter: async () =>
			(await import("@loams-plugins/plugin-forgejo-adapter"))
				.ForgejoAdapterService as unknown as AdapterCtor,
		configFrom: (url, x) => ({ baseUrl: base(url), token: x["token"] }),
	},
	zulip: {
		label: "Zulip",
		credentialFields: [f("email", "Email", false), f("apiKey", "API key"), sso],
		hasPanels: true,
		adapter: async () =>
			(await import("@loams-plugins/plugin-zulip-adapter"))
				.ZulipAdapterService as unknown as AdapterCtor,
		configFrom: (url, x) => ({
			baseUrl: base(url),
			email: x["email"],
			apiKey: x["apiKey"],
		}),
	},
	plane: {
		label: "Plane (ItsAPlan)",
		credentialFields: [
			f("apiKey", "API key"),
			f("projectKey", "Project key", false),
			// ItsAPlan's web UI and its API are separate origins: the embedded UI uses this.
			f("appUrl", "App URL (optional)", false),
			sso,
		],
		hasPanels: true,
		adapter: async () =>
			(await import("@loams-plugins/plugin-itsaplan-adapter"))
				.ItsAPlanAdapterService as unknown as AdapterCtor,
		configFrom: (url, x) => ({ baseUrl: base(url), apiKey: x["apiKey"] }),
	},
	glitchtip: {
		label: "GlitchTip",
		credentialFields: [f("token", "API token"), sso],
		hasPanels: true,
		adapter: async () =>
			(await import("@loams-plugins/plugin-glitchtip-adapter"))
				.GlitchtipAdapterService as unknown as AdapterCtor,
		configFrom: (url, x) => ({ baseUrl: base(url), token: x["token"] }),
	},
	openpanel: {
		label: "OpenPanel",
		credentialFields: [
			f("clientId", "Client ID"),
			f("clientSecret", "Client secret"),
			f("projectId", "Project ID", false),
			sso,
		],
		hasPanels: true,
		adapter: async () =>
			(await import("@loams-plugins/plugin-openpanel-adapter"))
				.OpenPanelAdapterService as unknown as AdapterCtor,
		configFrom: (url, x) => ({
			baseUrl: base(url),
			clientId: x["clientId"],
			clientSecret: x["clientSecret"],
		}),
	},
	matomo: {
		label: "Matomo",
		credentialFields: [
			f("apiToken", "API token"),
			f("idSite", "Site ID", false),
			sso,
		],
		hasPanels: true,
		adapter: async () =>
			(await import("@loams-plugins/plugin-matomo-adapter"))
				.MatomoAdapterService as unknown as AdapterCtor,
		configFrom: (url, x) => ({ baseUrl: base(url), apiToken: x["apiToken"] }),
	},
	langfuse: {
		label: "Langfuse",
		credentialFields: [
			f("publicKey", "Public key"),
			f("secretKey", "Secret key"),
			sso,
		],
		hasPanels: true,
		adapter: async () =>
			(await import("@loams-plugins/plugin-langfuse-adapter"))
				.LangfuseAdapterService as unknown as AdapterCtor,
		configFrom: (url, x) => ({
			baseUrl: base(url),
			publicKey: x["publicKey"],
			secretKey: x["secretKey"],
		}),
	},
	// SigNoz, the MIT community build, storing its telemetry in the install's
	// Loams House (D1666). A preview until SigNoz passes the House gate (D1571).
	signoz: {
		label: "SigNoz",
		preview: "Preview: SigNoz on Loams House",
		credentialFields: [sso],
		hasPanels: false,
		// Its unauthenticated health route sits at the origin, whatever path the UI is on.
		probe: async (url) => {
			const r = await fetch(new URL("/api/v2/healthz", url).href, {
				signal: AbortSignal.timeout(10_000),
			});
			if (!r.ok)
				throw Object.assign(new Error(`health check answered ${r.status}`), {
					status: r.status,
				});
		},
		configFrom: (url) => ({ baseUrl: base(url) }),
	},
};

export const FACTORY_APP_IDS = Object.keys(FACTORY_APPS) as FactoryAppId[];

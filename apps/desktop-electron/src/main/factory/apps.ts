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
	/** Absent for OpenObserve: full UI only. */
	adapter?: () => Promise<AdapterCtor>;
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
	openobserve: {
		label: "OpenObserve",
		credentialFields: [sso],
		hasPanels: false,
		configFrom: (url) => ({ baseUrl: base(url) }),
	},
};

export const FACTORY_APP_IDS = Object.keys(FACTORY_APPS) as FactoryAppId[];

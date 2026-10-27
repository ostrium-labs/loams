// The factory's apps as Electron main knows them: ids, labels and the read ops the
// agents' `factory_query` tool may name. Their connections, credentials and ops run
// in loams-agentd (SF1 amendment F1; design §50 §13, D795).
import type { FactoryAppId } from "../../shared/contracts";

export const FACTORY_LABELS: Record<FactoryAppId, string> = {
	forgejo: "Forgejo",
	zulip: "Zulip",
	plane: "Plane (ItsAPlan)",
	glitchtip: "GlitchTip",
	openpanel: "OpenPanel",
	matomo: "Matomo",
	langfuse: "Langfuse",
	signoz: "SigNoz",
};

export const FACTORY_APP_IDS = Object.keys(FACTORY_LABELS) as FactoryAppId[];

export const isFactoryApp = (a: unknown): a is FactoryAppId =>
	typeof a === "string" && Object.hasOwn(FACTORY_LABELS, a);

/** The daemon's read ops per app (`health` aside); SigNoz has none. */
export const FACTORY_READ_OPS: Partial<Record<FactoryAppId, string[]>> = {
	forgejo: ["repos", "issues", "version"],
	zulip: ["streams", "messages", "server"],
	plane: ["stats", "issues"],
	glitchtip: ["organizations", "issues", "latestEvent"],
	openpanel: ["insights"],
	matomo: ["visits", "pages"],
	langfuse: ["traces", "daily"],
};

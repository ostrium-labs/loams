import { z } from "zod";
import type { FactoryAppId } from "../../shared/contracts";
import type { Adapter } from "./apps";

export interface OpContext {
	/** Non-secret configured fields (projectKey, projectId, idSite, ...). */
	fields: Record<string, string>;
}
export interface Op {
	params: z.ZodTypeAny;
	run(adapter: Adapter, params: never, ctx: OpContext): Promise<unknown>;
}
type Rec = Record<string, unknown>;

const MAX_LIMIT = 50;
export const OP_NAME_ALLOWLIST = /^(get|list|search|health|fetch|query)/;

// Param schemas are strict: anything not declared is rejected (§19.5).
const limit = z.number().int().min(1).optional();
const page = z.number().int().min(1).optional();
const clamp = (n: number | undefined, dflt = 20): number =>
	Math.min(Math.max(n ?? dflt, 1), MAX_LIMIT);

const rec = (x: unknown): Rec =>
	typeof x === "object" && x !== null ? (x as Rec) : {};
const arr = (x: unknown): Rec[] => (Array.isArray(x) ? x.map(rec) : []);
const str = (x: unknown): string | undefined =>
	typeof x === "string" ? x : undefined;
const num = (x: unknown): number | undefined =>
	typeof x === "number" && Number.isFinite(x) ? x : undefined;
const numbers = (x: unknown): Record<string, number> => {
	const out: Record<string, number> = {};
	for (const [k, v] of Object.entries(rec(x))) {
		const n = num(v);
		if (n !== undefined) out[k] = n;
	}
	return out;
};

/** Thrown for a missing non-secret field; mapped to `bad_params` by the host. */
export class ParamError extends Error {}
const need = (v: string | undefined, name: string): string => {
	if (!v)
		throw new ParamError(`${name} is required (set it in the app settings)`);
	return v;
};

/** An upstream "never throws" status() result, rethrown so the host maps it. */
function assertStatusOk(r: unknown): void {
	const s = rec(r);
	if (s["ok"] === false) {
		throw Object.assign(new Error(str(s["error"]) ?? "unreachable"), {
			status: num(s["status"]) ?? 0,
		});
	}
}

const none = z.object({}).strict();
const q = z.string().max(200).optional();

const health = (run: (a: Adapter, c: OpContext) => Promise<unknown>): Op => ({
	params: none,
	run: async (a, _p, c) => {
		await run(a, c);
		return { status: "ok" };
	},
});

const issueDto = (i: Rec) => ({
	id: str(i["identifier"]) ?? str(i["id"]) ?? String(i["number"] ?? ""),
	title: str(i["title"]),
	state: str(i["state"]) ?? str(i["status"]),
	url: str(i["html_url"]) ?? str(i["permalink"]),
	updatedAt: str(i["updated_at"]) ?? str(i["updatedAt"]),
});

export const OPS: Record<FactoryAppId, Record<string, Op>> = {
	forgejo: {
		repos: {
			params: z.object({ q, page, limit }).strict(),
			run: async (a, p: { q?: string; page?: number; limit?: number }) =>
				arr(
					rec(
						await a["searchRepositories"]?.(p.q ?? "", {
							page: p.page,
							limit: clamp(p.limit),
						}),
					)["data"],
				).map((r) => ({
					fullName: str(r["full_name"]),
					description: str(r["description"]),
					stars: num(r["stars_count"]),
					forks: num(r["forks_count"]),
					openIssues: num(r["open_issues_count"]),
					updatedAt: str(r["updated_at"]),
					url: str(r["html_url"]),
				})),
		},
		issues: {
			params: z
				.object({
					q,
					page,
					limit,
					type: z.enum(["issues", "pulls"]).optional(),
					owner: z.string().min(1).max(100).optional(),
					repo: z.string().min(1).max(100).optional(),
				})
				.strict()
				.refine((p) => p.type !== "pulls" || (p.owner && p.repo), {
					message: "owner and repo are required for pulls",
				}),
			run: async (
				a,
				p: {
					q?: string;
					page?: number;
					limit?: number;
					type?: string;
					owner?: string;
					repo?: string;
				},
			) => {
				const opts = { page: p.page, limit: clamp(p.limit) };
				const res =
					p.type === "pulls"
						? await a["listPullRequests"]?.(p.owner, p.repo, {
								state: "open",
								...opts,
							})
						: await a["searchIssues"]?.(p.q ?? "", opts);
				return arr(rec(res)["items"]).map(issueDto);
			},
		},
		version: {
			params: none,
			run: async (a) => ({
				version: str(rec(await a["getVersion"]?.())["version"]),
			}),
		},
		health: health((a) => a["getVersion"]?.() as Promise<unknown>),
	},
	zulip: {
		streams: {
			params: none,
			run: async (a) =>
				arr(await a["listStreams"]?.())
					.slice(0, 200)
					.map((s) => ({
						id: num(s["stream_id"]),
						name: str(s["name"]),
						description: str(s["description"]),
						private: s["invite_only"] === true,
					})),
		},
		messages: {
			params: z.object({ channel: z.string().min(1).max(100), limit }).strict(),
			run: async (a, p: { channel: string; limit?: number }) =>
				arr(
					rec(
						await a["fetchMessages"]?.({
							narrow: [{ operator: "channel", operand: p.channel }],
							anchor: "newest",
							num_before: clamp(p.limit),
							num_after: 0,
						}),
					)["messages"],
				).map((m) => ({
					id: num(m["id"]),
					sender: str(m["sender_full_name"]),
					timestamp: num(m["timestamp"]),
					topic: str(m["subject"]) ?? str(m["topic"]),
					content: (str(m["content"]) ?? "").slice(0, 2000),
				})),
		},
		server: {
			params: none,
			run: async (a) => {
				const me = rec(await a["getSelf"]?.());
				return { name: str(me["full_name"]), email: str(me["email"]) };
			},
		},
		health: health((a) => a["getSelf"]?.() as Promise<unknown>),
	},
	plane: {
		stats: {
			params: z
				.object({ projectKey: z.string().min(1).max(50).optional() })
				.strict(),
			run: async (a, p: { projectKey?: string }, c) =>
				numbers(
					await a["getStats"]?.(
						need(p.projectKey ?? c.fields["projectKey"], "projectKey"),
					),
				),
		},
		issues: {
			params: z
				.object({ projectKey: z.string().min(1).max(50).optional(), limit })
				.strict(),
			run: async (a, p: { projectKey?: string; limit?: number }, c) =>
				arr(
					await a["listIssues"]?.(
						need(p.projectKey ?? c.fields["projectKey"], "projectKey"),
						{ limit: clamp(p.limit) },
					),
				).map((i) => ({
					id: str(i["identifier"]),
					title: str(i["title"]),
					priority: str(i["priority"]),
					updatedAt: str(i["updatedAt"]),
				})),
		},
		health: health(async (a) => {
			await a["health"]?.();
			const me = rec(await a["me"]?.());
			if (me["authenticated"] === false)
				throw Object.assign(new Error("not authenticated"), { status: 401 });
		}),
	},
	glitchtip: {
		organizations: {
			params: none,
			run: async (a) =>
				arr(rec(await a["listOrganizations"]?.())["data"]).map((o) => ({
					slug: str(o["slug"]),
					name: str(o["name"]),
				})),
		},
		issues: {
			params: z
				.object({
					orgSlug: z.string().min(1).max(100),
					limit,
					sort: z.enum(["date", "new", "freq", "priority"]).optional(),
				})
				.strict(),
			run: async (a, p: { orgSlug: string; limit?: number; sort?: string }) =>
				arr(
					rec(
						await a["listIssues"]?.(p.orgSlug, {
							query: "is:unresolved",
							limit: clamp(p.limit),
							sort: p.sort,
						}),
					)["data"],
				)
					.filter(
						(i) => i["status"] === undefined || i["status"] === "unresolved",
					)
					.map((i) => ({
						id: str(i["id"]),
						title: str(i["title"]),
						level: str(i["level"]),
						count: str(i["count"]) ?? num(i["count"]),
						lastSeen: str(i["lastSeen"]),
						permalink: str(i["permalink"]),
					})),
		},
		health: health((a) => a["root"]?.() as Promise<unknown>),
	},
	openpanel: {
		insights: {
			params: z
				.object({
					projectId: z.string().min(1).max(100).optional(),
					range: z.string().max(20).optional(),
					interval: z
						.enum(["minute", "hour", "day", "week", "month"])
						.optional(),
					limit,
				})
				.strict(),
			run: async (
				a,
				p: {
					projectId?: string;
					range?: string;
					interval?: string;
					limit?: number;
				},
				c,
			) => {
				const id = need(p.projectId ?? c.fields["projectId"], "projectId");
				const [ov, pages] = await Promise.all([
					a["overview"]?.(id, { range: p.range, interval: p.interval }),
					a["topPages"]?.(id, { range: p.range, limit: clamp(p.limit, 10) }),
				]);
				const o = rec(ov);
				return {
					summary: numbers(o["summary"]),
					series: arr(o["series"])
						.slice(0, 400)
						.map(
							(s) => numbers(s) as Record<string, number> & { date?: string },
						)
						.map((n, i) => ({
							...n,
							date: str(arr(o["series"])[i]?.["date"]),
						})),
					topPages: arr(pages).map((r) => ({
						path: str(r["path"]),
						sessions: num(r["sessions"]),
						pageviews: num(r["pageviews"]),
					})),
				};
			},
		},
		health: health(async (a, c) => {
			await a["health"]?.();
			const id = c.fields["projectId"];
			if (id) await a["live"]?.(id);
		}),
	},
	matomo: {
		visits: {
			params: z
				.object({
					idSite: z.string().regex(/^\d+$/).optional(),
					period: z.enum(["day", "week", "month", "year"]).optional(),
					date: z
						.string()
						.regex(
							/^(today|yesterday|last\d{1,3}|\d{4}-\d{2}-\d{2}(,\d{4}-\d{2}-\d{2})?)$/,
						)
						.optional(),
				})
				.strict(),
			run: async (
				a,
				p: { idSite?: string; period?: string; date?: string },
				c,
			) => {
				const r = rec(
					await a["getVisitsSummary"]?.({
						idSite: need(p.idSite ?? c.fields["idSite"], "idSite"),
						period: p.period ?? "day",
						date: p.date ?? "last7",
					}),
				);
				return arr(r["rows"]).map((row) => ({
					date: str(row["date"]) ?? str(row["label"]),
					...numbers(row),
				}));
			},
		},
		pages: {
			params: z
				.object({
					idSite: z.string().regex(/^\d+$/).optional(),
					period: z.enum(["day", "week", "month", "year"]).optional(),
					date: z
						.string()
						.regex(/^(today|yesterday|last\d{1,3}|\d{4}-\d{2}-\d{2})$/)
						.optional(),
					limit,
				})
				.strict(),
			run: async (
				a,
				p: { idSite?: string; period?: string; date?: string; limit?: number },
				c,
			) => {
				const r = rec(
					await a["getPageUrls"]?.({
						idSite: need(p.idSite ?? c.fields["idSite"], "idSite"),
						period: p.period ?? "day",
						date: p.date ?? "today",
						rowLimit: clamp(p.limit, 10),
					}),
				);
				return arr(r["rows"]).map((row) => ({
					label: str(row["label"]),
					hits: num(row["nb_hits"]),
					visits: num(row["nb_visits"]),
				}));
			},
		},
		health: health((a) => a["getVersion"]?.() as Promise<unknown>),
	},
	langfuse: {
		traces: {
			params: z
				.object({ limit, fromStartTime: z.string().max(40).optional() })
				.strict(),
			run: async (a, p: { limit?: number; fromStartTime?: string }) =>
				arr(
					rec(
						await a["listObservations"]?.({
							isRootObservation: true,
							limit: clamp(p.limit),
							fromStartTime: p.fromStartTime,
						}),
					)["data"],
				).map((o) => ({
					id: str(o["id"]),
					traceId: str(o["traceId"]),
					name: str(o["name"]),
					startTime: str(o["startTime"]),
					level: str(o["level"]),
					totalCost: num(o["totalCost"]) ?? num(o["calculatedTotalCost"]),
				})),
		},
		daily: {
			params: none,
			run: async (a) => {
				try {
					const r = rec(await a["metricsDaily"]?.());
					return {
						available: true,
						days: arr(r["data"])
							.slice(0, 120)
							.map((d) => ({ date: str(d["date"]), ...numbers(d) })),
					};
				} catch (e) {
					// Undocumented endpoint: 404 means "unavailable on this version".
					if (rec(e)["status"] === 404) return { available: false, days: [] };
					throw e;
				}
			},
		},
		health: health(async (a) => {
			assertStatusOk(await a["status"]?.());
			try {
				await a["listObservations"]?.({ limit: 1 });
			} catch (e) {
				if (rec(e)["status"] !== 404) throw e;
			}
		}),
	},
	openobserve: {},
};

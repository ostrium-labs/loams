import { readFileSync } from "node:fs";
import { describe, expect, it } from "vite-plus/test";
import { FACTORY_APPS } from "../src/main/factory/apps";
import {
	OP_NAME_ALLOWLIST,
	OPS,
	UnsupportedError,
} from "../src/main/factory/ops";

const src = readFileSync(
	new URL("../src/main/factory/ops.ts", import.meta.url),
	"utf8",
);
// Per-app section of the OPS table: `\t<app>: {` at depth 1 up to the next one.
function sections(): Record<string, string> {
	const body = src.slice(src.indexOf("export const OPS"));
	const out: Record<string, string> = {};
	const re = /^\t(\w+): \{/gm;
	const marks = [...body.matchAll(re)];
	marks.forEach((m, i) => {
		out[m[1] ?? ""] = body.slice(m.index, marks[i + 1]?.index ?? body.length);
	});
	return out;
}
const names = (text: string) =>
	[...text.matchAll(/call\(\s*a,\s*"([A-Za-z]+)"/g)].map((m) => m[1] ?? "");

describe("ops", () => {
	it("ops_are_read_only_names", () => {
		const called = names(src);
		expect(called.length).toBeGreaterThan(15);
		const allowedExtra = new Set([
			"me",
			"root",
			"status",
			"overview",
			"topPages",
			"live",
			"metricsDaily",
			// Langfuse 4's daily usage (/v2/metrics): a read.
			"metrics",
			// GlitchTip's latest event of an issue (SD1 Task 27): a read.
			"latestIssueEvent",
		]);
		for (const m of called)
			expect(OP_NAME_ALLOWLIST.test(m) || allowedExtra.has(m), m).toBe(true);
		expect(
			called.some((m) =>
				/^(create|delete|update|set|post|put|patch|send)/i.test(m),
			),
		).toBe(false);
	});

	it("every_call_has_a_literal_method_name", () => {
		const body = src.slice(src.indexOf("export const OPS"));
		expect((body.match(/\bcall\(/g) ?? []).length).toBe(names(body).length);
	});

	it("ops_invoke_adapters_only_through_call", () => {
		const body = src.slice(src.indexOf("export const OPS"));
		expect(body).not.toMatch(/\ba\.[A-Za-z]/);
		expect(body).not.toMatch(/\ba\[/);
		expect(body).not.toMatch(/\.(apply|call|bind)\(/);
	});

	it("every_method_exists_on_the_real_adapter", async () => {
		const secs = sections();
		for (const [id, def] of Object.entries(FACTORY_APPS)) {
			if (!def.adapter) continue;
			const Ctor = await def.adapter();
			for (const m of names(secs[id] ?? "")) {
				expect(typeof Ctor.prototype[m], `${id}.${m}`).toBe("function");
			}
			expect(names(secs[id] ?? "").length, id).toBeGreaterThan(0);
		}
	});

	it("every_adapter_app_has_health", () => {
		for (const [id, def] of Object.entries(FACTORY_APPS))
			expect(Object.keys(OPS[id as keyof typeof OPS]).includes("health")).toBe(
				!!def.adapter,
			);
	});

	it("every_app_has_optional_sso_origin", () => {
		for (const def of Object.values(FACTORY_APPS))
			expect(def.credentialFields).toContainEqual({
				key: "ssoOrigin",
				label: "SSO origin (optional)",
				secret: false,
			});
	});
});

// SD1 Task 27: the reads behind Studio's in-context links (§53 §5.9).
describe("ops for Studio", () => {
	const adapter = (methods: Record<string, (...a: unknown[]) => unknown>) =>
		Object.fromEntries(
			Object.entries(methods).map(([k, f]) => [
				k,
				async (...a: unknown[]) => f(...a),
			]),
		) as never;
	const ctx = { fields: {} };

	it("forgejo_pulls_carry_the_head_branch", async () => {
		const op = OPS.forgejo.issues;
		if (!op) throw new Error("no issues op");
		const params = op.params.parse({
			type: "pulls",
			owner: "acme",
			repo: "site",
		});
		const rows = (await op.run(
			adapter({
				listPullRequests: () => ({
					items: [
						{
							number: 12,
							title: "Fix login",
							state: "open",
							html_url: "https://git.example.test/acme/site/pulls/12",
							head: { ref: "mkt-42-fix", sha: "abc" },
						},
					],
				}),
			}),
			params as never,
			ctx,
		)) as { id: string; head?: string }[];
		expect(rows).toEqual([
			expect.objectContaining({ id: "12", head: "mkt-42-fix" }),
		]);
	});

	it("glitchtip_latest_event_projects_stack_and_release", async () => {
		const op = OPS.glitchtip.latestEvent;
		if (!op) throw new Error("no latestEvent op");
		expect(() => op.params.parse({ issueId: "../x" })).toThrow();
		expect(() => op.params.parse({ issueId: "1", extra: 1 })).toThrow();
		const seen: unknown[] = [];
		const frames = Array.from({ length: 80 }, (_, i) => ({
			filename: `src/f${i}.ts`,
			function: `fn${i}`,
			lineNo: i + 1,
		}));
		const out = (await op.run(
			adapter({
				latestIssueEvent: (id) => {
					seen.push(id);
					return {
						culprit: "checkout/cart.ts in total",
						release: { version: "web@1.4.2" },
						entries: [
							{ type: "breadcrumbs", data: {} },
							{
								type: "exception",
								data: {
									values: [
										{
											type: "TypeError",
											value: "Cannot read properties of undefined",
											stacktrace: { frames },
										},
									],
								},
							},
						],
					};
				},
			}),
			op.params.parse({ issueId: "311" }) as never,
			ctx,
		)) as { release?: string; culprit?: string; stack?: string };
		expect(seen).toEqual(["311"]);
		expect(out.release).toBe("web@1.4.2");
		expect(out.culprit).toBe("checkout/cart.ts in total");
		expect(out.stack).toMatch(
			/^TypeError: Cannot read properties of undefined\n/,
		);
		// The innermost frame (last in Sentry's order) comes first; the trace is capped.
		expect(out.stack).toContain("  at fn79 (src/f79.ts:80)");
		expect(out.stack?.length).toBeLessThanOrEqual(4000);
	});

	it("glitchtip_release_from_tags", async () => {
		const op = OPS.glitchtip.latestEvent;
		if (!op) throw new Error("no latestEvent op");
		const out = (await op.run(
			adapter({
				latestIssueEvent: () => ({
					tags: [{ key: "release", value: "api@2" }],
				}),
			}),
			op.params.parse({ issueId: "1" }) as never,
			ctx,
		)) as { release?: string; stack?: string };
		expect(out.release).toBe("api@2");
		expect(out.stack).toBeUndefined();
	});
	it("langfuse_traces_fall_back_to_the_legacy_list_on_404", async () => {
		// A default self-hosted Langfuse 3.x has no /v2/observations: read its trace list.
		const op = OPS.langfuse.traces;
		if (!op) throw new Error("no traces op");
		const notFound = () =>
			Promise.reject(Object.assign(new Error("404"), { status: 404 }));
		const out = await op.run(
			adapter({
				listObservations: notFound,
				listLegacyTraces: (q: unknown) => {
					expect(q).toMatchObject({ limit: 20 });
					return {
						data: [
							{
								id: "t1",
								name: "checkout-agent",
								timestamp: "2026-10-18T08:00:00Z",
								totalCost: 0.5,
							},
						],
					};
				},
			}),
			op.params.parse({}) as never,
			ctx,
		);
		expect(out).toEqual([
			{
				id: "t1",
				traceId: "t1",
				name: "checkout-agent",
				startTime: "2026-10-18T08:00:00Z",
				level: undefined,
				totalCost: 0.5,
			},
		]);
	});

	it("langfuse_traces_without_either_api_is_unsupported_not_a_raw_404", async () => {
		const op = OPS.langfuse.traces;
		if (!op) throw new Error("no traces op");
		const notFound = () =>
			Promise.reject(
				Object.assign(
					new Error("GET http://lf/api/public/v2/observations failed with 404"),
					{ status: 404 },
				),
			);
		const run = op.run(
			adapter({ listObservations: notFound, listLegacyTraces: notFound }),
			op.params.parse({}) as never,
			ctx,
		);
		await expect(run).rejects.toBeInstanceOf(UnsupportedError);
		await expect(run).rejects.not.toThrow(/http:/);
	});

	it("langfuse_daily_falls_back_to_v2_metrics_on_404", async () => {
		// Langfuse 4 serves no /metrics/daily: the daily table comes from /v2/metrics.
		const op = OPS.langfuse.daily;
		if (!op) throw new Error("no daily op");
		const asked: Record<string, unknown>[] = [];
		const out = await op.run(
			adapter({
				metricsDaily: () =>
					Promise.reject(Object.assign(new Error("404"), { status: 404 })),
				metrics: (q: unknown) => {
					asked.push(q as Record<string, unknown>);
					return {
						data: [
							{
								time_dimension: "2026-10-18",
								count_count: 3,
								sum_totalCost: 0.5,
							},
							{
								time_dimension: "2026-10-19",
								count_count: 0,
								sum_totalCost: null,
							},
						],
					};
				},
			}),
			op.params.parse({}) as never,
			ctx,
		);
		expect(asked[0]).toMatchObject({
			view: "observations",
			timeDimension: { granularity: "day" },
		});
		const from = Date.parse(String(asked[0]?.["fromTimestamp"]));
		const to = Date.parse(String(asked[0]?.["toTimestamp"]));
		expect(to - from).toBe(30 * 86_400_000);
		expect(out).toEqual({
			available: true,
			days: [
				{ date: "2026-10-18", observations: 3, totalCost: 0.5 },
				{ date: "2026-10-19", observations: 0 },
			],
		});
	});

	it("openpanel_insights_default_to_a_window_that_includes_today", async () => {
		// OpenPanel's own default window ends yesterday: a deploy's first hours read as zero.
		const op = OPS.openpanel.insights;
		if (!op) throw new Error("no insights op");
		const seen: unknown[] = [];
		await op.run(
			adapter({
				overview: (_id: unknown, q: unknown) => {
					seen.push(q);
					return { summary: {}, series: [] };
				},
				topPages: (_id: unknown, q: unknown) => {
					seen.push(q);
					return [];
				},
			}),
			op.params.parse({ projectId: "shop" }) as never,
			ctx,
		);
		expect(seen).toEqual([
			expect.objectContaining({ range: "30d" }),
			expect.objectContaining({ range: "30d" }),
		]);
	});
});

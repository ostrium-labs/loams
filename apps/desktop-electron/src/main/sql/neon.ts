// Client for the Neon pageserver management API and the safekeeper HTTP API of the dev stack
// (deploy/neon). Routes checked against deploy/neon/README.md; see docs in the Task 22 report.
import { randomBytes } from "node:crypto";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import type {
	PgTimeline,
	PgWalStatus,
} from "../../shared/contracts";
import { SqlError } from "./caps";

export interface PgBranchInput {
	name: string;
	ancestorTimelineId: string;
	ancestorStartLsn?: string;
}

export const PAGESERVER_URL = "http://127.0.0.1:9898";
export const SAFEKEEPER_URL = "http://127.0.0.1:7676";

const HEX32 = /^[0-9a-f]{32}$/;
const LSN = /^[0-9A-Fa-f]{1,8}\/[0-9A-Fa-f]{1,8}$/;

export function assertId(kind: string, v: unknown): string {
	if (typeof v !== "string" || !HEX32.test(v))
		throw new SqlError("invalid", `${kind} must be 32 hex characters`);
	return v;
}

export type FetchFn = (
	url: string,
	init?: {
		method?: string;
		headers?: Record<string, string>;
		body?: string;
		signal?: AbortSignal;
	},
) => Promise<{ ok: boolean; status: number; text(): Promise<string> }>;

export interface NeonOpts {
	pageserverUrl?: string;
	safekeeperUrl?: string;
	fetch?: FetchFn;
	/** `<userData>/postgres/branches.json`: timeline id -> branch name. */
	branchesFile: string;
	newId?: () => string;
	timeoutMs?: number;
}

type Obj = Record<string, unknown>;
const str = (v: unknown) => (typeof v === "string" ? v : undefined);

export class NeonClient {
	private readonly ps: string;
	private readonly sk: string;
	private readonly fetchFn: FetchFn;

	constructor(private readonly o: NeonOpts) {
		this.ps = o.pageserverUrl ?? PAGESERVER_URL;
		this.sk = o.safekeeperUrl ?? SAFEKEEPER_URL;
		this.fetchFn = o.fetch ?? (globalThis.fetch as unknown as FetchFn);
	}

	private async call(
		base: string,
		path: string,
		init?: { method: string; body: unknown },
	): Promise<unknown> {
		let res: Awaited<ReturnType<FetchFn>>;
		try {
			res = await this.fetchFn(base + path, {
				method: init?.method ?? "GET",
				headers: init ? { "Content-Type": "application/json" } : undefined,
				body: init ? JSON.stringify(init.body) : undefined,
				signal: AbortSignal.timeout(this.o.timeoutMs ?? 10_000),
			});
		} catch (e) {
			throw new SqlError(
				"unreachable",
				`cannot reach ${base} (is the postgres stack running?): ${(e as Error).message}`,
			);
		}
		const text = await res.text();
		if (!res.ok)
			throw new SqlError(
				`http_${res.status}`,
				text.slice(0, 500) || `HTTP ${res.status}`,
			);
		return text ? JSON.parse(text) : null;
	}

	private names(): Record<string, string> {
		try {
			const v = JSON.parse(
				readFileSync(this.o.branchesFile, "utf8"),
			) as unknown;
			return v && typeof v === "object" ? (v as Record<string, string>) : {};
		} catch {
			return {};
		}
	}

	private saveName(id: string, name: string): void {
		const all = { ...this.names(), [id]: name };
		mkdirSync(dirname(this.o.branchesFile), { recursive: true });
		writeFileSync(this.o.branchesFile, `${JSON.stringify(all, null, 2)}\n`);
	}

	/** GET /v1/tenant */
	async tenants(): Promise<string[]> {
		const rows = (await this.call(this.ps, "/v1/tenant")) as Obj[];
		return (rows ?? []).map((r) => String(r.id));
	}

	private timeline(r: Obj, names: Record<string, string>): PgTimeline {
		const id = String(r.timeline_id);
		return {
			timelineId: id,
			name: names[id],
			ancestorTimelineId: str(r.ancestor_timeline_id),
			ancestorLsn: str(r.ancestor_lsn),
			lastRecordLsn: str(r.last_record_lsn) ?? "0/0",
			state: str(r.state),
		};
	}

	/** GET /v1/tenant/{t}/timeline */
	async timelines(tenant: string): Promise<PgTimeline[]> {
		const t = assertId("tenant", tenant);
		const rows = (await this.call(
			this.ps,
			`/v1/tenant/${t}/timeline`,
		)) as Obj[];
		const names = this.names();
		return (rows ?? []).map((r) => this.timeline(r, names));
	}

	/** POST /v1/tenant/{t}/timeline/ with a random new timeline id; the name is kept locally. */
	async createBranch(tenant: string, b: PgBranchInput): Promise<PgTimeline> {
		const t = assertId("tenant", tenant);
		const ancestor = assertId("ancestorTimelineId", b.ancestorTimelineId);
		const name = typeof b.name === "string" ? b.name.trim() : "";
		if (!name || name.length > 128)
			throw new SqlError("invalid", "branch name must be 1-128 characters");
		if (b.ancestorStartLsn !== undefined && !LSN.test(b.ancestorStartLsn))
			throw new SqlError(
				"invalid",
				"ancestorStartLsn must look like 0/16B3748",
			);
		const id = (this.o.newId ?? (() => randomBytes(16).toString("hex")))();
		const body = createBranchBody(id, ancestor, b.ancestorStartLsn);
		const info = (await this.call(this.ps, `/v1/tenant/${t}/timeline/`, {
			method: "POST",
			body,
		})) as Obj;
		this.saveName(id, name);
		return this.timeline(
			{ ...info, timeline_id: info?.timeline_id ?? id },
			this.names(),
		);
	}

	/** GET /v1/tenant/{t}/timeline/{tl} on the safekeeper (7676). */
	async walStatus(tenant: string, timeline: string): Promise<PgWalStatus> {
		const t = assertId("tenant", tenant);
		const tl = assertId("timeline", timeline);
		const r = (await this.call(
			this.sk,
			`/v1/tenant/${t}/timeline/${tl}`,
		)) as Obj;
		return {
			timelineId: tl,
			flushLsn: str(r.flush_lsn) ?? "0/0",
			commitLsn: str(r.commit_lsn) ?? "0/0",
		};
	}
}

export function createBranchBody(
	newTimelineId: string,
	ancestorTimelineId: string,
	ancestorStartLsn?: string,
): Record<string, string> {
	const body: Record<string, string> = {
		new_timeline_id: newTimelineId,
		ancestor_timeline_id: ancestorTimelineId,
	};
	if (ancestorStartLsn) body.ancestor_start_lsn = ancestorStartLsn;
	return body;
}

export function branchesFile(userData: string): string {
	return join(userData, "postgres", "branches.json");
}

import { dialog, ipcMain } from "electron";
import { CH, type LoamsDesktopApi } from "../../shared/contracts";
import { assertTrustedSender } from "../security/policy";
import { getMainWindow } from "../shell/main-window";
import { isPlainRead, SqlError, toResult } from "./caps";
import { branchesFile, NeonClient } from "./neon";
import { createPgBackend, type PostgresBackend, type SqlBackend } from "./pg";
import { createWesqlBackend, type MySqlBackend } from "./wesql";

export interface SqlServices {
	pg: PostgresBackend;
	wesql: MySqlBackend;
}

export function createSqlServices(userData: string): SqlServices {
	const neon = new NeonClient({ branchesFile: branchesFile(userData) });
	return { pg: createPgBackend({ neon }), wesql: createWesqlBackend() };
}

async function confirmWrite(label: string, sql: string): Promise<boolean> {
	const opts = {
		type: "warning" as const,
		buttons: ["Cancel", "Run"],
		defaultId: 0,
		cancelId: 0,
		title: `Run on ${label}?`,
		message: `This statement is not a plain read and may change data in ${label}.`,
		detail: sql.length > 1200 ? `${sql.slice(0, 1200)}...` : sql,
	};
	const w = getMainWindow();
	const r =
		w && !w.isDestroyed()
			? await dialog.showMessageBox(w, opts)
			: await dialog.showMessageBox(opts);
	return r.response === 1;
}

const arg = (v: unknown, name: string): string => {
	if (typeof v !== "string" || !v)
		throw new SqlError("invalid", `${name} must be a non-empty string`);
	return v;
};

function branchInput(v: unknown): Parameters<LoamsDesktopApi["pg"]["createBranch"]>[1] {
	const o = (v ?? {}) as Record<string, unknown>;
	return {
		name: arg(o.name, "name"),
		ancestorTimelineId: arg(o.ancestorTimelineId, "ancestorTimelineId"),
		ancestorStartLsn:
			typeof o.ancestorStartLsn === "string" ? o.ancestorStartLsn : undefined,
	};
}

export function registerSqlIpc(
	svc: SqlServices,
	confirm: (label: string, sql: string) => Promise<boolean> = confirmWrite,
): void {
	const h = <A extends unknown[], T>(
		ch: string,
		fn: (...a: A) => Promise<T>,
		secrets: () => string[] = () => [],
	) =>
		ipcMain.handle(ch, async (e, ...a: unknown[]) => {
			assertTrustedSender(e);
			return toResult(() => fn(...(a as A)), secrets());
		});

	const raw = (ch: string, fn: () => unknown) =>
		ipcMain.handle(ch, (e) => {
			assertTrustedSender(e);
			return fn();
		});
	const query = (label: string, b: SqlBackend) => async (sql: unknown) => {
		const text = arg(sql, "sql");
		if (isPlainRead(text)) return b.query(text, { readOnly: true });
		if (!(await confirm(label, text)))
			throw new SqlError("cancelled", "cancelled");
		return b.query(text, { readOnly: false });
	};

	h(CH.pgTenants, () => svc.pg.tenants());
	h(CH.pgTimelines, (t: unknown) => svc.pg.timelines(arg(t, "tenant")));
	h(CH.pgCreateBranch, (t: unknown, b: unknown) =>
		svc.pg.createBranch(arg(t, "tenant"), branchInput(b)),
	);
	h(CH.pgWalStatus, (t: unknown, tl: unknown) =>
		svc.pg.walStatus(arg(t, "tenant"), arg(tl, "timeline")),
	);
	// Plan contract: connection() and revealPassword() resolve to the bare value (no IpcResult).
	raw(CH.pgConnection, () => svc.pg.connection());
	raw(CH.pgRevealPassword, () => svc.pg.password());
	h(CH.pgQuery, query("Postgres", svc.pg), () => [svc.pg.password()]);
	raw(CH.wesqlConnection, () => svc.wesql.connection());
	raw(CH.wesqlRevealPassword, () => svc.wesql.password());
	h(CH.wesqlSchemas, () => svc.wesql.schemas());
	h(CH.wesqlTables, (s: unknown) => svc.wesql.tables(arg(s, "schema")));
	h(CH.wesqlQuery, query("WeSQL", svc.wesql), () => [svc.wesql.password()]);
}

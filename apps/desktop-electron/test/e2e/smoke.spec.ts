import { execFileSync } from "node:child_process";
import {
	chmodSync,
	existsSync,
	mkdirSync,
	mkdtempSync,
	rmSync,
	writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import {
	type ElectronApplication,
	_electron as electron,
	expect,
	type Page,
	test,
} from "@playwright/test";

const appRoot = resolve(__dirname, "../..");
const fakeEngine = join(appRoot, "test/fixtures/fake-engine.mjs");

/**
 * Which engine to run. LOAMS_E2E_ENGINE: `real` (a missing binary fails), `fake`, or `auto`
 * (default: the real engine when it exists, else the fake). A LOAMS_BIN that is set but
 * missing always throws, so CI cannot fall back to the fake by accident.
 */
function resolveEngine(): string | undefined {
	const mode = process.env.LOAMS_E2E_ENGINE ?? "auto";
	if (!["real", "fake", "auto"].includes(mode))
		throw new Error(`LOAMS_E2E_ENGINE must be real, fake or auto, got ${mode}`);
	if (mode === "fake") return undefined;
	const exe = process.platform === "win32" ? "loams.exe" : "loams";
	const explicit = process.env.LOAMS_BIN;
	if (explicit) {
		if (!existsSync(explicit))
			throw new Error(`LOAMS_BIN is set but missing: ${explicit}`);
		return explicit;
	}
	let found: string | undefined;
	try {
		const meta = JSON.parse(
			execFileSync(
				"cargo",
				["metadata", "--format-version", "1", "--no-deps"],
				{ cwd: appRoot, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 },
			),
		) as { target_directory: string };
		const bin = join(meta.target_directory, "release", exe);
		if (existsSync(bin)) found = bin;
	} catch {
		// no cargo: handled below
	}
	if (!found && mode === "real")
		throw new Error(
			"LOAMS_E2E_ENGINE=real but no release `loams` was found (build with --features live,durable or set LOAMS_BIN)",
		);
	return found;
}

/** An executable that runs the fake engine; `dev --help` answers at once (the live probe). */
function fakeEngineWrapper(dir: string): string {
	if (process.platform === "win32") {
		const f = join(dir, "loams.cmd");
		writeFileSync(
			f,
			`@echo off\r\necho %* | findstr /C:"--help" >nul && exit /b 0\r\n"${process.execPath}" "${fakeEngine}" %*\r\n`,
		);
		return f;
	}
	const f = join(dir, "loams");
	writeFileSync(
		f,
		`#!/bin/sh\ncase "$*" in *--help*) exit 0;; esac\nexec "${process.execPath}" "${fakeEngine}" "$@"\n`,
	);
	chmodSync(f, 0o755);
	return f;
}

function alive(pid: number): boolean {
	try {
		process.kill(pid, 0);
		return true;
	} catch (e) {
		return (e as NodeJS.ErrnoException).code === "EPERM";
	}
}

async function until(
	fn: () => boolean,
	ms: number,
): Promise<number | undefined> {
	const t0 = Date.now();
	while (Date.now() - t0 < ms) {
		if (fn()) return Date.now() - t0;
		await new Promise((r) => setTimeout(r, 50));
	}
	return undefined;
}

interface PageGlobal {
	loamsDesktop?: {
		engine: { state(): Promise<{ phase: string; pid?: number }> };
		update: { state(): Promise<{ phase: string }> };
	};
	location: { hash: string };
}
// page.evaluate callbacks are serialised, so each one casts globalThis (the window) inline.

/** page.evaluate that survives a navigation in flight ("Execution context was destroyed"). */
async function ev<T>(page: Page, fn: () => T | Promise<T>): Promise<T> {
	for (let i = 0; ; i++) {
		try {
			await page.waitForLoadState("domcontentloaded");
			return await page.evaluate(fn);
		} catch (e) {
			if (i >= 4 || !/context was destroyed|navigation/i.test(String(e)))
				throw e;
		}
	}
}

const hash = (page: Page): Promise<string> =>
	ev(page, () => (globalThis as unknown as PageGlobal).location.hash);

/** A Connect unary JSON call through the app protocol (and so the main-process proxy). */
async function rpc(
	page: Page,
	path: string,
	body: unknown,
): Promise<{ status: number; json: unknown }> {
	return page.evaluate(
		async ([p, b]) => {
			const r = await globalThis.fetch(`loams-app://console${p}`, {
				method: "POST",
				headers: {
					"content-type": "application/json",
					"connect-protocol-version": "1",
				},
				body: JSON.stringify(b),
			});
			const text = await r.text();
			let json: unknown = text;
			try {
				json = JSON.parse(text);
			} catch {}
			return { status: r.status, json };
		},
		[path, body] as [string, unknown],
	);
}

test("electron smoke", async () => {
	const real = resolveEngine();
	const useReal = real !== undefined;
	test.info().annotations.push({
		type: "engine",
		description: useReal ? "real" : "fake",
	});
	const work = mkdtempSync(join(tmpdir(), "loams-e2e-"));
	const userData = join(work, "user-data");
	mkdirSync(userData);
	const bin = real ?? fakeEngineWrapper(work);
	const timings: Record<string, number> = {};
	const t0 = Date.now();
	let app: ElectronApplication | undefined;
	let appPid = 0;
	let enginePid = 0;
	try {
		const env: Record<string, string> = {};
		for (const [k, v] of Object.entries(process.env))
			if (v !== undefined && k !== "ELECTRON_RUN_AS_NODE") env[k] = v;
		env.LOAMS_BIN = bin;
		env.LOAMS_DESKTOP_USER_DATA = userData;
		const extra = (process.env.LOAMS_E2E_ELECTRON_ARGS ?? "")
			.split(/\s+/)
			.filter(Boolean);
		app = await electron.launch({
			args: [...extra, appRoot],
			cwd: appRoot,
			env,
		});
		appPid = app.process().pid ?? 0;
		const page = await app.firstWindow();
		timings.window = Date.now() - t0;

		// 1. The native window title (the page sets its own document title).
		const title = await app.evaluate(({ BrowserWindow }) =>
			BrowserWindow.getAllWindows()[0]?.getTitle(),
		);
		expect(title).toBe("Loams Desktop");
		expect(page.url()).toContain("loams-app://console/ui/cordis.html");

		// 2. The server switcher shows Local with a ready engine.
		const switcher = page.locator(".lc-server-switch");
		await expect(switcher).toContainText("Local", { timeout: 60_000 });
		await expect(switcher.getByRole("img", { name: /ready/i })).toBeVisible({
			timeout: 60_000,
		});
		timings.engineReady = Date.now() - t0;
		const engine = await ev(page, () =>
			(() => {
				const d = (globalThis as unknown as PageGlobal).loamsDesktop;
				if (!d) throw new Error("no desktop api");
				return d.engine.state();
			})(),
		);
		expect(engine.phase).toBe("ready");
		if (engine.phase !== "ready") throw new Error("engine not ready");
		enginePid = engine.pid ?? 0;
		expect(enginePid).toBeGreaterThan(0);
		expect(alive(enginePid)).toBe(true);

		// Updater (built main bundle): a dev build has no feed, so the update state
		// IPC answers "disabled". That proves the updater module and the externalized
		// @noble/ed25519 (ESM) loaded in main.
		const upd = await ev(page, () =>
			(() => {
				const d = (globalThis as unknown as PageGlobal).loamsDesktop;
				if (!d) throw new Error("no desktop api");
				return d.update.state();
			})(),
		);
		expect(upd.phase).toBe("disabled");

		// 3. Data page.
		await ev(page, () => {
			(globalThis as unknown as PageGlobal).location.hash = "#/data";
		});
		// The fake engine reports no APIs, so the page only has a data plane with the real one.
		if (useReal) {
			await expect(page.getByLabel("Namespace")).toBeVisible();
			const ns = await rpc(
				page,
				"/loams.collection.v1.NamespaceService/CreateNamespace",
				{ namespace: "e2e" },
			);
			expect(ns.status, JSON.stringify(ns.json)).toBe(200);
			const coll = await rpc(
				page,
				"/loams.collection.v1.CollectionService/CreateCollection",
				{
					namespace: "e2e",
					name: "docs",
					schema: {
						fields: [
							{
								name: "body",
								source_path: "body",
								kind: { text: { analyzer: "standard", positions: true } },
								indexed: true,
								fast: false,
							},
						],
						vectors: [],
						sparse_vectors: [],
						dynamic: "ignore",
						max_fields: 1000,
					},
				},
			);
			expect(coll.status, JSON.stringify(coll.json)).toBe(200);
			const texts = ["hello world", "hello again", "something else"];
			const wr = await rpc(
				page,
				"/loams.collection.v1.DocumentService/WriteDocuments",
				{
					namespace: "e2e",
					collection: "docs",
					idempotencyKey: "e2e-1",
					ops: texts.map((body, i) => ({
						upsert: { id: { uint: String(i + 1) }, source: { body } },
					})),
				},
			);
			expect(wr.status, JSON.stringify(wr.json)).toBe(200);
			await expect
				.poll(
					async () => {
						const r = await rpc(
							page,
							"/loams.collection.v1.QueryService/Search",
							{
								namespace: "e2e",
								collection: "docs",
								retrievers: [
									{
										text: {
											query: { match: { field: "body", text: "hello" } },
											k: 10,
										},
									},
								],
								limit: 10,
							},
						);
						return (r.json as { hits?: unknown[] }).hits?.length ?? 0;
					},
					{ timeout: 30_000, intervals: [250, 500, 1000] },
				)
				.toBeGreaterThanOrEqual(1);
			timings.data = Date.now() - t0;
		}

		// 4. Software Factory: all eight apps start unconfigured.
		await ev(page, () => {
			(globalThis as unknown as PageGlobal).location.hash = "#/factory";
		});
		const tiles = page.locator("article", { hasText: "Not configured" });
		await expect(tiles).toHaveCount(8);

		// 5. A loams:// deep link (second-instance simulation) opens Servers.
		await app.evaluate(({ app: a }) => {
			a.emit(
				"second-instance",
				{},
				["loams-desktop", "loams://open/servers"],
				"",
			);
		});
		await expect.poll(() => hash(page)).toBe("#/settings/servers");

		// 6. Quit: the engine and the app exit within 6 s (assert by pid).
		const tq = Date.now();
		void app.evaluate(({ app: a }) => a.quit()).catch(() => undefined);
		const gone = await until(() => !alive(enginePid), 6_000);
		expect(gone, "engine pid still alive 6 s after quit").toBeDefined();
		const appGone = await until(() => !alive(appPid), 10_000);
		expect(appGone, "app pid still alive after quit").toBeDefined();
		timings.quit = Date.now() - tq;
		app = undefined;
	} finally {
		if (app) await app.close().catch(() => undefined);
		if (enginePid && alive(enginePid)) {
			try {
				process.kill(enginePid, "SIGKILL");
			} catch {}
		}
		rmSync(work, { recursive: true, force: true });
		console.log(
			`[e2e] ${useReal ? "real" : "fake"} engine timings (ms): ${JSON.stringify(timings)}`,
		);
	}
});

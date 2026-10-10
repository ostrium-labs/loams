import { mkdirSync, mkdtempSync, rmSync } from "node:fs";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import {
	_electron as electron,
	expect,
	type Page,
	test,
} from "@playwright/test";

const appRoot = resolve(__dirname, "../..");

interface DesktopGlobal {
	loamsDesktop: {
		servers: {
			add(e: { name: string; kind: string; url: string }): Promise<{
				ok: boolean;
				value: { id: string };
			}>;
			activate(id: string): Promise<unknown>;
		};
	};
}

/**
 * page.evaluate that survives the reload an activate triggers ("Execution context was
 * destroyed"). Re-running activate for the same server is a no-op, so a retry is safe.
 */
async function ev<T>(
	page: Page,
	fn: (a: string) => T | Promise<T>,
	arg: string,
): Promise<T> {
	for (let i = 0; ; i++) {
		try {
			await page.waitForLoadState("domcontentloaded");
			return await page.evaluate(fn, arg);
		} catch (e) {
			if (i >= 4 || !/context was destroyed|navigation/i.test(String(e)))
				throw e;
			await page.waitForTimeout(200);
		}
	}
}

// I4: leaving a server drops its session, including path-scoped (`Path=/api`) and Secure
// cookies. Chromium stores no cookies for loams-app://; the proxy's session.fetch stores
// them under the server's own host, which is what must be cleared.
test("leaving a server clears its path-scoped and secure cookies", async () => {
	const srv = createServer((_req, res) => {
		res.writeHead(200, {
			"content-type": "application/json",
			"set-cookie": ["sid=1; Path=/; HttpOnly", "csrf=2; Path=/api; Secure"],
		});
		res.end("{}");
	});
	await new Promise<void>((r) => srv.listen(0, "127.0.0.1", r));
	const port = (srv.address() as AddressInfo).port;
	const work = mkdtempSync(join(tmpdir(), "loams-ck-"));
	const userData = join(work, "user-data");
	mkdirSync(userData);
	const env: Record<string, string> = {};
	for (const [k, v] of Object.entries(process.env))
		if (v !== undefined && k !== "ELECTRON_RUN_AS_NODE") env[k] = v;
	env.LOAMS_BIN = join(work, "no-engine"); // no engine needed
	env.LOAMS_DESKTOP_USER_DATA = userData;
	const extra = (process.env.LOAMS_E2E_ELECTRON_ARGS ?? "")
		.split(/\s+/)
		.filter(Boolean);
	const app = await electron.launch({
		args: [...extra, appRoot],
		cwd: appRoot,
		env,
	});
	try {
		const page = await app.firstWindow();
		await page.waitForLoadState("domcontentloaded");
		const jar = () =>
			app.evaluate(async ({ session }) =>
				(await session.defaultSession.cookies.get({}))
					.map(
						(c) => `${c.domain}${c.path} ${c.name}${c.secure ? " secure" : ""}`,
					)
					.sort(),
			);
		await app.evaluate(({ session }) =>
			session.defaultSession.cookies.set({
				url: "https://other.example/",
				name: "keep",
				value: "1",
			}),
		);
		const added = await ev(
			page,
			(url: string) =>
				(globalThis as unknown as DesktopGlobal).loamsDesktop.servers.add({
					name: "probe",
					kind: "remote",
					url,
				}),
			`http://127.0.0.1:${port}`,
		);
		expect(added.ok).toBe(true);
		const activate = (id: string) =>
			ev(
				page,
				(sid: string) =>
					(
						globalThis as unknown as DesktopGlobal
					).loamsDesktop.servers.activate(sid),
				id,
			);
		await activate(added.value.id);
		// Through the app protocol and its proxy, from main: no renderer navigation to race.
		await app.evaluate(async ({ session }) => {
			await session.defaultSession.fetch("loams-app://console/api/v1/x");
		});
		await expect
			.poll(jar)
			.toEqual([
				"127.0.0.1/ sid",
				"127.0.0.1/api csrf secure",
				"other.example/ keep",
			]);

		await activate("demo");
		await expect.poll(jar).toEqual(["other.example/ keep"]);
	} finally {
		await app.close().catch(() => undefined);
		srv.close();
		rmSync(work, { recursive: true, force: true });
	}
});

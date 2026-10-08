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

async function settle(page: Page): Promise<void> {
	await page.waitForTimeout(500);
	await page.waitForLoadState("domcontentloaded");
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
		const added = await page.evaluate(
			(url) =>
				(globalThis as unknown as DesktopGlobal).loamsDesktop.servers.add({
					name: "probe",
					kind: "remote",
					url,
				}),
			`http://127.0.0.1:${port}`,
		);
		expect(added.ok).toBe(true);
		await page.evaluate(
			(id) =>
				(globalThis as unknown as DesktopGlobal).loamsDesktop.servers.activate(
					id,
				),
			added.value.id,
		);
		await settle(page);
		await page.evaluate(async () => {
			await fetch("loams-app://console/api/v1/x", { credentials: "include" });
		});
		await expect
			.poll(jar)
			.toEqual([
				"127.0.0.1/ sid",
				"127.0.0.1/api csrf secure",
				"other.example/ keep",
			]);

		await page.evaluate(() =>
			(globalThis as unknown as DesktopGlobal).loamsDesktop.servers.activate(
				"demo",
			),
		);
		await expect.poll(jar).toEqual(["other.example/ keep"]);
	} finally {
		await app.close().catch(() => undefined);
		srv.close();
		rmSync(work, { recursive: true, force: true });
	}
});

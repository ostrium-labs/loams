import { ipcMain } from "electron";
import {
	CH,
	type FactoryAppId,
	type FactoryQuery,
	type IpcResult,
} from "../../shared/contracts";
import { assertTrustedSender } from "../security/policy";
import { FACTORY_APPS } from "./apps";
import type { FactoryHost } from "./host";
import type { FactoryViews } from "./views.electron";

const isApp = (a: unknown): a is FactoryAppId =>
	typeof a === "string" && Object.hasOwn(FACTORY_APPS, a);
const bad = (): IpcResult<never> => ({
	ok: false,
	code: "bad_request",
	message: "Invalid request",
});

export function registerFactoryIpc(
	host: FactoryHost,
	views: FactoryViews,
): void {
	ipcMain.handle(CH.factoryList, async (e) => {
		assertTrustedSender(e);
		return host.list();
	});
	ipcMain.handle(
		CH.factoryConfigure,
		async (e, app: unknown, url: unknown, fields: unknown) => {
			assertTrustedSender(e);
			if (
				!isApp(app) ||
				typeof url !== "string" ||
				typeof fields !== "object" ||
				fields === null
			)
				return bad();
			const r = await host.configure(
				app,
				url,
				fields as Record<string, string>,
			);
			// The window holds the old URL/SSO origin: close it so the next open is fresh.
			if (r.ok) views.close(app);
			return r;
		},
	);
	ipcMain.handle(CH.factoryTest, async (e, app: unknown) => {
		assertTrustedSender(e);
		if (!isApp(app)) throw new Error("unknown app");
		return host.test(app);
	});
	ipcMain.handle(CH.factoryRemove, async (e, app: unknown) => {
		assertTrustedSender(e);
		if (isApp(app)) {
			views.close(app);
			await host.remove(app);
		}
	});
	ipcMain.handle(CH.factoryOpen, async (e, app: unknown) => {
		assertTrustedSender(e);
		if (!isApp(app)) return bad();
		return views.open(app);
	});
	ipcMain.handle(CH.factoryClose, async (e, app: unknown) => {
		assertTrustedSender(e);
		if (isApp(app)) views.close(app);
	});
	ipcMain.handle(CH.factoryQuery, async (e, q: unknown) => {
		assertTrustedSender(e);
		if (typeof q !== "object" || q === null) return bad();
		return host.query(q as FactoryQuery);
	});
}

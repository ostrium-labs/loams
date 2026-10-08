// Adapted from dataelement/dsh-desktop (MIT), src/main/close-to-tray.ts.
import { join } from "node:path";
import { app, Menu, nativeImage, Tray } from "electron";
import type { EngineState, ServerEntry } from "../../shared/contracts";
import { trayModel } from "./tray-model";

export interface TrayDeps {
	getEngine: () => EngineState;
	getServer: () => ServerEntry;
	onAction: (id: string) => void;
}

export interface TrayHandle {
	setBadge(n: number): void;
	refresh(): void;
	destroy(): void;
}

/** Creates the tray; undefined when the platform has none (logged, not fatal). */
export function createTray(deps: TrayDeps): TrayHandle | undefined {
	let tray: Tray;
	try {
		const dir = join(app.getAppPath(), "build");
		const icon = nativeImage
			.createFromPath(join(dir, "tray-32.png"))
			.resize({ width: 16, height: 16 });
		tray = new Tray(icon);
	} catch (e) {
		console.warn("tray unavailable:", (e as Error).message);
		return undefined;
	}
	let badge = 0;
	const refresh = (): void => {
		if (tray.isDestroyed()) return;
		const m = trayModel(deps.getEngine(), badge, deps.getServer());
		tray.setToolTip(m.tooltip);
		tray.setContextMenu(
			Menu.buildFromTemplate(
				m.items.map((i) => ({
					label: i.label,
					enabled: i.enabled,
					click: () => deps.onAction(i.id),
				})),
			),
		);
	};
	tray.on("click", () => deps.onAction("open"));
	refresh();
	console.log("tray created");
	return {
		setBadge(n) {
			badge = n;
			refresh();
		},
		refresh,
		destroy() {
			if (!tray.isDestroyed()) tray.destroy();
		},
	};
}

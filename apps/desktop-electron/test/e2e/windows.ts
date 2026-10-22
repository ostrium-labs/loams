import type { ElectronApplication, Page } from "@playwright/test";

/** The console window (the splash opens first since CF1 Task 15). */
export async function mainWindow(app: ElectronApplication): Promise<Page> {
	for (let i = 0; i < 200; i++) {
		const page = app.windows().find((w) => w.url().startsWith("loams-app://console"));
		if (page) return page;
		await new Promise((r) => setTimeout(r, 100));
	}
	throw new Error("the console window did not open");
}

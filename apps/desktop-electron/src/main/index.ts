import { execFileSync } from "node:child_process";
import { join } from "node:path";
import { app, BrowserWindow } from "electron";
import { resolvePaths } from "./paths";

let cachedTargetDir: string | undefined | null = null;

/** CARGO_TARGET_DIR if set, else (dev only) `cargo metadata`'s target_directory, cached. */
function cargoTargetDir(): string | undefined {
	if (process.env.CARGO_TARGET_DIR) return process.env.CARGO_TARGET_DIR;
	if (app.isPackaged) return undefined;
	if (cachedTargetDir !== null) return cachedTargetDir;
	try {
		const out = execFileSync(
			"cargo",
			["metadata", "--format-version", "1", "--no-deps"],
			{
				cwd: join(app.getAppPath(), "..", ".."),
				encoding: "utf8",
				timeout: 15_000,
				maxBuffer: 64 * 1024 * 1024,
			},
		);
		cachedTargetDir = (JSON.parse(out) as { target_directory?: string })
			.target_directory;
	} catch {
		cachedTargetDir = undefined;
	}
	return cachedTargetDir;
}

export function appPaths() {
	return resolvePaths({
		userData: app.getPath("userData"),
		logs: app.getPath("logs"),
		resourcesPath: process.resourcesPath,
		isPackaged: app.isPackaged,
		appRoot: app.getAppPath(),
		loamsBin: process.env.LOAMS_BIN,
		cargoTargetDir: cargoTargetDir(),
	});
}

function createWindow(): BrowserWindow {
	const win = new BrowserWindow({
		width: 1280,
		height: 800,
		show: false,
		title: "Loams Desktop",
		webPreferences: {
			preload: join(__dirname, "../preload/index.cjs"),
			contextIsolation: true,
			sandbox: true,
			nodeIntegration: false,
			webSecurity: true,
			webviewTag: false,
		},
	});
	win.once("ready-to-show", () => win.show());
	// Placeholder; Task 2 replaces this with loams-app://console/ui/cordis.html.
	void win.loadURL("https://example.invalid");
	return win;
}

if (!app.requestSingleInstanceLock()) {
	app.quit();
} else {
	app.on("second-instance", () => {
		const [win] = BrowserWindow.getAllWindows();
		if (win) {
			if (win.isMinimized()) win.restore();
			win.focus();
		}
	});
	void app.whenReady().then(() => {
		app.setAppUserModelId("dev.loams.desktop");
		createWindow();
		app.on("activate", () => {
			if (BrowserWindow.getAllWindows().length === 0) createWindow();
		});
	});
	app.on("window-all-closed", () => {
		if (process.platform !== "darwin") app.quit();
	});
}

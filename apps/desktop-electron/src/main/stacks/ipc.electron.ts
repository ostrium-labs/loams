import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { delimiter, join } from "node:path";
import { ipcMain } from "electron";
import { CH, type StackId, type StackState } from "../../shared/contracts";
import { assertTrustedSender } from "../security/policy";
import { getMainWindow } from "../shell/main-window";
import { detectRuntime } from "./runtime";
import {
	POLL_MS,
	STACK_IDS,
	StackManager,
	type StackManagerDeps,
} from "./stacks";

/** Looks a binary up on PATH (with .exe/.cmd handling on Windows) without a shell. */
export function whichBin(bin: string): string | null {
	const exts =
		process.platform === "win32" ? ["", ".exe", ".cmd", ".bat"] : [""];
	for (const dir of (process.env.PATH ?? "").split(delimiter)) {
		if (!dir) continue;
		for (const ext of exts) {
			const p = join(dir, bin + ext);
			if (existsSync(p)) return p;
		}
	}
	return null;
}

/** `docker compose` needs the plugin, not just the docker binary. */
function composeWorks(bin: string, args: string[]): boolean {
	try {
		execFileSync(bin, [...args, "version"], {
			stdio: "ignore",
			timeout: 15_000,
		});
		return true;
	} catch {
		return false;
	}
}

export function createStackManager(
	deps: Pick<StackManagerDeps, "stacksDir" | "logsDir">,
): StackManager {
	let runtime = detectRuntime(whichBin);
	if (runtime && !composeWorks(runtime.bin, runtime.args)) {
		// Fall through the remaining candidates when the preferred one lacks compose.
		const skip = runtime.bin;
		runtime = detectRuntime((b) => (b === skip ? null : whichBin(b)));
		if (runtime && !composeWorks(runtime.bin, runtime.args)) runtime = null;
	}
	return new StackManager({ ...deps, runtime });
}

function isStackId(v: unknown): v is StackId {
	return typeof v === "string" && (STACK_IDS as string[]).includes(v);
}

export function registerStacksIpc(manager: StackManager): void {
	const bad = () => ({
		ok: false as const,
		code: "invalid",
		message: "unknown stack",
	});
	ipcMain.handle(CH.stacksState, async (e, id: unknown) => {
		assertTrustedSender(e);
		if (!isStackId(id)) return { phase: "stopped" } satisfies StackState;
		return manager.state(id);
	});
	ipcMain.handle(CH.stacksStart, async (e, id: unknown) => {
		assertTrustedSender(e);
		return isStackId(id) ? manager.start(id) : bad();
	});
	ipcMain.handle(CH.stacksStop, async (e, id: unknown) => {
		assertTrustedSender(e);
		return isStackId(id) ? manager.stop(id) : bad();
	});
	manager.on("state", (id: StackId, s: StackState) => {
		getMainWindow()?.webContents.send(CH.stacksEvent, id, s);
	});
	// Poll every 5 s while a page is open; the first poll also adopts stacks left running.
	void manager.poll();
	setInterval(() => {
		const w = getMainWindow();
		if (w && !w.isDestroyed()) void manager.poll();
	}, POLL_MS).unref();
}

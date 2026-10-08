import { execFile } from "node:child_process";
import { existsSync } from "node:fs";
import { delimiter, join } from "node:path";
import { ipcMain, shell } from "electron";
import { CH, type StackId, type StackState } from "../../shared/contracts";
import { assertTrustedSender } from "../security/policy";
import { getMainWindow } from "../shell/main-window";
import { type ComposeRuntime, detectRuntime } from "./runtime";
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

/** Picks the first usable runtime; the compose plugin is probed asynchronously. */
export async function resolveRuntime(
	which: (bin: string) => string | null,
	works: (rt: ComposeRuntime) => Promise<boolean>,
): Promise<ComposeRuntime | null> {
	const skipped = new Set<string>();
	for (;;) {
		const rt = detectRuntime((b) => (skipped.has(b) ? null : which(b)));
		if (!rt) return null;
		if (await works(rt)) return rt;
		skipped.add(rt.bin);
	}
}

/** `docker compose` needs the plugin, not just the docker binary. */
function composeWorks(rt: ComposeRuntime): Promise<boolean> {
	return new Promise((resolve) => {
		execFile(
			rt.bin,
			[...rt.args, "version"],
			{ timeout: 15_000, windowsHide: true },
			(err) => resolve(!err),
		);
	});
}

/** The runtime is resolved lazily on first use; creating the manager does no I/O. */
export function createStackManager(
	deps: Pick<
		StackManagerDeps,
		"stacksDir" | "logsDir" | "sourceDir" | "version" | "alwaysCopy"
	>,
): StackManager {
	return new StackManager({
		...deps,
		runtime: () => resolveRuntime(whichBin, composeWorks),
	});
}

function isStackId(v: unknown): v is StackId {
	return typeof v === "string" && (STACK_IDS as string[]).includes(v);
}

export function registerStacksIpc(
	manager: StackManager,
	logsDir: string,
): void {
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
	ipcMain.handle(CH.stacksOpenLogs, async (e, id: unknown) => {
		assertTrustedSender(e);
		if (!isStackId(id)) return bad();
		const file = join(logsDir, "stacks", `${id}.log`);
		if (!existsSync(file))
			return { ok: false as const, code: "no_log", message: "no log yet" };
		const err = await shell.openPath(file);
		return err
			? { ok: false as const, code: "open_failed", message: err }
			: { ok: true as const, value: undefined };
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

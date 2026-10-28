// The bundled daemon on each platform (0.1.0 release fix B9): the launcher picks the platform's
// executable from resolvePaths, spawns it hidden with the same arguments everywhere, and stops it
// with SIGTERM, or with a plain kill (TerminateProcess) on Windows, where Node has no signals.
// Runs on every OS: the child process is a fake, so nothing platform-specific is executed.
import { EventEmitter } from "node:events";
import { existsSync, mkdirSync, mkdtempSync, rmSync, statSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import type { ChildProcess, spawn } from "node:child_process";
import { afterEach, beforeEach, describe, expect, it } from "vite-plus/test";
import { createAgentdLauncher } from "../src/main/agentd/launch";
import { AGENTD_PROTOCOL } from "../src/main/agentd/types";
import { resolvePaths } from "../src/main/paths";

// Scratch under the app (never /tmp), as release-scripts.test.ts does.
const scratchRoot = resolve(__dirname, "..", ".scratch");
let dir = "";
beforeEach(() => {
	mkdirSync(scratchRoot, { recursive: true });
	dir = mkdtempSync(join(scratchRoot, "launch-"));
});
afterEach(() => rmSync(dir, { recursive: true, force: true }));

interface Spawned {
	file: string;
	args: readonly string[];
	options: Record<string, unknown>;
	kills: (NodeJS.Signals | number | undefined)[];
}

/** A spawn that writes the daemon's discovery file, as `run --child` does once it listens. */
function fakeSpawn(agentdDir: string, spawned: Spawned[]): typeof spawn {
	return ((file: string, args: readonly string[], options: Record<string, unknown>) => {
		const kills: Spawned["kills"] = [];
		spawned.push({ file, args, options, kills });
		const c = new EventEmitter() as ChildProcess & { exitCode: number | null; signalCode: null };
		Object.assign(c, {
			exitCode: null,
			signalCode: null,
			kill: (signal?: NodeJS.Signals | number) => {
				kills.push(signal);
				return true;
			},
		});
		writeFileSync(
			join(agentdDir, "agentd.json"),
			JSON.stringify({
				protocol: AGENTD_PROTOCOL,
				version: "0.1.0",
				pid: process.pid,
				port: 40000,
				connectPort: 40001,
				startedAtMs: Date.now(),
				mode: "child",
			}),
		);
		return c;
	}) as unknown as typeof spawn;
}

describe("agentd launcher on each platform", () => {
	for (const [platform, exe] of [
		["linux", "loams-agentd"],
		["darwin", "loams-agentd"],
		["win32", "loams-agentd.exe"],
	] as const) {
		it(`starts_and_stops_the_bundled_daemon_on_${platform}`, async () => {
			const resources = join(dir, "Loams Desktop", "resources");
			const paths = resolvePaths({
				userData: join(dir, "userData"),
				logs: join(dir, "logs"),
				resourcesPath: resources,
				isPackaged: true,
				appRoot: join(dir, "app"),
				platform,
			});
			const bundled = join(resources, "bin", exe);
			const agentdDir = join(dir, "userData", "agentd");
			const spawned: Spawned[] = [];
			const l = createAgentdLauncher({
				dir: agentdDir,
				logsDir: paths.logs,
				candidates: paths.agentdBin,
				exists: (p) => p === bundled || existsSync(p),
				spawnChild: fakeSpawn(agentdDir, spawned),
				readyTimeoutMs: 2_000,
				platform,
			});
			expect(l.binary()).toBe(bundled);

			await l.launch("child");
			const cfg = join(agentdDir, "config.toml");
			expect(spawned).toHaveLength(1);
			const [s0] = spawned;
			if (!s0) throw new Error("not spawned");
			expect(s0.file).toBe(bundled);
			expect(s0.args).toEqual(["run", "--child", "--config", cfg]);
			expect(s0.options.windowsHide).toBe(true);
			expect(existsSync(cfg)).toBe(true);
			expect(existsSync(join(paths.logs, "agentd.log"))).toBe(true);
			// Unix only: the daemon's directory is the owner's alone (Windows uses the daemon's DACLs).
			if (platform !== "win32" && process.platform !== "win32")
				expect(statSync(agentdDir).mode & 0o777).toBe(0o700);

			l.killChild();
			expect(s0.kills).toEqual([platform === "win32" ? undefined : "SIGTERM"]);
			l.killChild();
			expect(s0.kills).toHaveLength(1);
		});
	}

	it("says_not_bundled_when_the_platforms_executable_is_absent", async () => {
		// A Windows build whose resources hold only the Unix name has no daemon.
		const paths = resolvePaths({
			userData: join(dir, "userData"),
			logs: join(dir, "logs"),
			resourcesPath: join(dir, "resources"),
			isPackaged: true,
			appRoot: join(dir, "app"),
			platform: "win32",
		});
		const l = createAgentdLauncher({
			dir: join(dir, "userData", "agentd"),
			logsDir: paths.logs,
			candidates: paths.agentdBin,
			exists: (p) => p === join(dir, "resources", "bin", "loams-agentd"),
			platform: "win32",
		});
		expect(l.binary()).toBeNull();
		await expect(l.launch("child")).rejects.toMatchObject({ code: "unavailable" });
	});
});

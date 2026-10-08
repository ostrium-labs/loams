import { execFile } from "node:child_process";
import { join } from "node:path";
import { promisify } from "node:util";
import { app } from "electron";
import { type ResolvedPaths, resolvePaths } from "./paths";

const run = promisify(execFile);
let cachedTargetDir: Promise<string | undefined> | undefined;

/** CARGO_TARGET_DIR if set, else (dev only) `cargo metadata`'s target_directory, cached. */
function cargoTargetDir(): Promise<string | undefined> {
	if (process.env.CARGO_TARGET_DIR)
		return Promise.resolve(process.env.CARGO_TARGET_DIR);
	if (app.isPackaged) return Promise.resolve(undefined);
	cachedTargetDir ??= run(
		"cargo",
		["metadata", "--format-version", "1", "--no-deps"],
		{
			cwd: join(app.getAppPath(), "..", ".."),
			encoding: "utf8",
			timeout: 15_000,
			maxBuffer: 64 * 1024 * 1024,
		},
	)
		.then(
			({ stdout }) =>
				(JSON.parse(stdout) as { target_directory?: string }).target_directory,
		)
		.catch(() => undefined);
	return cachedTargetDir;
}

export async function appPaths(): Promise<ResolvedPaths> {
	return resolvePaths({
		userData: app.getPath("userData"),
		logs: app.getPath("logs"),
		resourcesPath: process.resourcesPath,
		isPackaged: app.isPackaged,
		appRoot: app.getAppPath(),
		loamsBin: process.env.LOAMS_BIN,
		cargoTargetDir: await cargoTargetDir(),
	});
}

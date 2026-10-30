import { join, resolve } from "node:path";

export interface PathsEnv {
	userData: string;
	logs: string;
	resourcesPath: string;
	isPackaged: boolean;
	/** The app directory (apps/desktop-electron); the repo root is two levels up. */
	appRoot: string;
	/** Value of LOAMS_BIN, when set. Tried first. */
	loamsBin?: string;
	/** Value of LOAMS_AGENTD_BIN, when set. Tried first for the agent daemon. */
	agentdBin?: string;
	/** Value of LOAMS_TRANSFER_BIN, when set. Tried first for the transfer sidecar. */
	transferBin?: string;
	/** Cargo target directory, injected by the caller. Defaults to `<repo>/target` in dev. */
	cargoTargetDir?: string;
	/** Defaults to process.platform. */
	platform?: NodeJS.Platform;
}

export interface ResolvedPaths {
	engineData: string;
	logs: string;
	consoleDist: string;
	/** Candidates in order of preference. */
	engineBin: string[];
	/** loams-agentd candidates, in order of preference. */
	agentdBin: string[];
	/** loams-transfer (the transfer sidecar, §85) candidates, in order of preference. */
	transferBin: string[];
	factoryDir: string;
	serversFile: string;
	/** The shipped stacks: loams-postgres-dev/, wesql/ and tikv/ (each with a compose.yaml). Read-only. */
	stacksDir: string;
	/** Writable copies of the stacks that compose runs from (userData/stacks). */
	stacksRunDir: string;
}

export function resolvePaths(env: PathsEnv): ResolvedPaths {
	const win = (env.platform ?? process.platform) === "win32";
	const exe = win ? "loams.exe" : "loams";
	const agentdExe = win ? "loams-agentd.exe" : "loams-agentd";
	const repoRoot = resolve(env.appRoot, "..", "..");
	const consoleDist = env.isPackaged
		? join(env.resourcesPath, "console")
		: join(repoRoot, "web", "apps", "console", "dist");

	const cargoTarget =
		env.cargoTargetDir ??
		(env.isPackaged ? undefined : join(repoRoot, "target"));
	const engineBin: string[] = [];
	if (env.loamsBin) engineBin.push(env.loamsBin);
	engineBin.push(join(env.resourcesPath, "bin", exe));
	if (cargoTarget) {
		engineBin.push(
			join(cargoTarget, "release", exe),
			join(cargoTarget, "debug", exe),
		);
	}

	const agentdBin: string[] = [];
	if (env.agentdBin) agentdBin.push(env.agentdBin);
	agentdBin.push(join(env.resourcesPath, "bin", agentdExe));
	if (cargoTarget) {
		agentdBin.push(
			join(cargoTarget, "release", agentdExe),
			join(cargoTarget, "debug", agentdExe),
		);
	}

	const transferExe = win ? "loams-transfer.exe" : "loams-transfer";
	const transferBin: string[] = [];
	if (env.transferBin) transferBin.push(env.transferBin);
	transferBin.push(join(env.resourcesPath, "bin", transferExe));
	if (cargoTarget) {
		transferBin.push(
			join(cargoTarget, "release", transferExe),
			join(cargoTarget, "debug", transferExe),
		);
	}

	return {
		engineData: join(env.userData, "engine"),
		logs: env.logs,
		consoleDist,
		engineBin,
		agentdBin,
		transferBin,
		factoryDir: join(env.userData, "factory"),
		serversFile: join(env.userData, "servers.json"),
		stacksDir: env.isPackaged
			? join(env.resourcesPath, "stacks")
			: join(repoRoot, "deploy"),
		stacksRunDir: join(env.userData, "stacks"),
	};
}

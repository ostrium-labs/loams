// Copies the engine binary into resources/bin/ for electron-builder (extraResources) and strips it.
// Source: LOAMS_BIN, else <cargo target_directory>/release/loams[.exe] from `cargo metadata`.
// Set LOAMS_BUILD_ENGINE=1 to run `cargo build --release -p loams --features live,durable,live-tikv,graph` first
// (the default build has live, on the embedded store, but lacks durable and live-tikv, which the
// desktop needs for durable execution and Live on the TiKV stack, ruling T23-7, and graph, which
// serves loams.graph.v1 to the Graph page, GR1 Task 8).
// Never sets CARGO_TARGET_DIR: the shared target dir comes from the cargo config.
//
// It also bundles loams-transfer, the sidecar behind Transfers (design §85): LOAMS_TRANSFER_BIN, else
// <target>/release/loams-transfer[.exe]. Without it the app still packages (the Transfers page then
// says it is not bundled), unless LOAMS_REQUIRE_TRANSFER=1.
//
// It also bundles loams-agentd, the daemon behind Studio (0.1.0 release fix B9): LOAMS_AGENTD_BIN, else
// <target>/release/loams-agentd[.exe]. With it come SHA256SUMS and VERSION, which `loams-agentd
// install-runtime` checks when the user turns background agents on. Without the daemon the app still
// packages (Studio then says it is not bundled), unless LOAMS_REQUIRE_AGENTD=1, as the release sets.
import { execFileSync, spawnSync } from "node:child_process";
import {
	chmodSync,
	copyFileSync,
	existsSync,
	mkdirSync,
	readFileSync,
	rmSync,
	statSync,
	writeFileSync,
} from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const { writeBinSums } = createRequire(import.meta.url)("./bin-sums.cjs");

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repo = resolve(root, "..", "..");

// Windows builds set this when the engine did not build or pass its smoke test: ship no engine, and
// the app shows the "not available on Windows yet" state. The agent daemon is still bundled below.
const noEngine = process.env.LOAMS_DESKTOP_NO_LOCAL_ENGINE === "1";
const exe = process.platform === "win32" ? "loams.exe" : "loams";

if (!noEngine && process.env.LOAMS_BUILD_ENGINE === "1") {
	execFileSync(
		"cargo",
		["build", "--release", "-p", "loams", "--features", "live,durable,live-tikv,graph"],
		{ cwd: repo, stdio: "inherit" },
	);
}

let targetDir;
const releaseDir = () => {
	if (!targetDir) {
		const meta = JSON.parse(
			execFileSync("cargo", ["metadata", "--format-version", "1", "--no-deps"], {
				cwd: repo,
				maxBuffer: 256 * 1024 * 1024,
			}).toString(),
		);
		targetDir = meta.target_directory;
	}
	return join(targetDir, "release");
};

// The unstripped release binary is ~470 MB. Strip the copy, never the cargo output.
function strip(file, what) {
	if (process.platform === "win32") return;
	const flags = process.platform === "darwin" ? ["-x"] : [];
	for (const tool of ["strip", "llvm-strip"]) {
		const r = spawnSync(tool, [...flags, file], { stdio: "ignore" });
		if (r.status === 0) return;
	}
	console.warn(`warning: no strip/llvm-strip found; shipping an unstripped ${what}`);
}

const outDir = join(root, "resources", "bin");
rmSync(outDir, { recursive: true, force: true });
mkdirSync(outDir, { recursive: true });

if (noEngine) {
	console.warn("LOAMS_DESKTOP_NO_LOCAL_ENGINE=1: not bundling the local engine");
} else {
	const src = process.env.LOAMS_BIN || join(releaseDir(), exe);
	if (!existsSync(src)) {
		console.error(
			`engine binary not found at ${src}\n` +
				"Build it: cargo build --release -p loams --features live,durable,live-tikv,graph (or set LOAMS_BIN / LOAMS_BUILD_ENGINE=1).",
		);
		process.exit(1);
	}
	const dest = join(outDir, exe);
	copyFileSync(src, dest);
	chmodSync(dest, 0o755);
	strip(dest, "engine");
	const mb = (statSync(dest).size / 1048576).toFixed(1);
	console.log(`engine: ${src} -> ${dest} (${mb} MB)`);
}

const agentdExe = process.platform === "win32" ? "loams-agentd.exe" : "loams-agentd";
let agentdSrc = process.env.LOAMS_AGENTD_BIN;
if (!agentdSrc) {
	try {
		agentdSrc = join(releaseDir(), agentdExe);
	} catch {
		agentdSrc = undefined;
	}
}
if (!agentdSrc || !existsSync(agentdSrc)) {
	const msg =
		`loams-agentd not found at ${agentdSrc ?? "<cargo target>/release"}\n` +
		"Build it: cargo build --release -p loams-agentd (or set LOAMS_AGENTD_BIN).";
	if (process.env.LOAMS_REQUIRE_AGENTD === "1") {
		console.error(msg);
		process.exit(1);
	}
	console.warn(`warning: ${msg}\nPackaging without the agent daemon: Studio will say it is not bundled.`);
} else {
	const agentdDest = join(outDir, agentdExe);
	copyFileSync(agentdSrc, agentdDest);
	chmodSync(agentdDest, 0o755);
	strip(agentdDest, "loams-agentd");
	// What `loams-agentd install-runtime --from resources/bin` checks. Signing rewrites the binaries,
	// so mac-sign.cjs (macOS) and the release workflow (Windows, after SignPath) write it again.
	writeBinSums(outDir);
	const version =
		process.env.LOAMS_DESKTOP_VERSION ||
		JSON.parse(readFileSync(join(root, "package.json"), "utf8")).version;
	writeFileSync(join(outDir, "VERSION"), `${version}\n`);
	const amb = (statSync(agentdDest).size / 1048576).toFixed(1);
	console.log(`agentd: ${agentdSrc} -> ${agentdDest} (${amb} MB), SHA256SUMS and VERSION ${version}`);
}

const transferExe = process.platform === "win32" ? "loams-transfer.exe" : "loams-transfer";
let transferSrc = process.env.LOAMS_TRANSFER_BIN;
if (!transferSrc) {
	try {
		transferSrc = join(releaseDir(), transferExe);
	} catch {
		transferSrc = undefined;
	}
}
if (!transferSrc || !existsSync(transferSrc)) {
	const msg =
		`loams-transfer not found at ${transferSrc ?? "<cargo target>/release"}\n` +
		"Build it: cargo build --release -p loams-transfer (or set LOAMS_TRANSFER_BIN).";
	if (process.env.LOAMS_REQUIRE_TRANSFER === "1") {
		console.error(msg);
		process.exit(1);
	}
	console.warn(`warning: ${msg}\nPackaging without the transfer sidecar: Transfers will say it is not bundled.`);
} else {
	const transferDest = join(outDir, transferExe);
	copyFileSync(transferSrc, transferDest);
	chmodSync(transferDest, 0o755);
	strip(transferDest, "loams-transfer");
	const tmb = (statSync(transferDest).size / 1048576).toFixed(1);
	console.log(`transfer: ${transferSrc} -> ${transferDest} (${tmb} MB)`);
}

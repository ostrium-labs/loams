// Copies the engine binary into resources/bin/ for electron-builder (extraResources) and strips it.
// Source: LOAMS_BIN, else <cargo target_directory>/release/loams[.exe] from `cargo metadata`.
// Set LOAMS_BUILD_ENGINE=1 to run `cargo build --release -p loams --features live,durable` first
// (the default build lacks the live and durable features the desktop needs).
// Never sets CARGO_TARGET_DIR: the shared target dir comes from the cargo config.
import { execFileSync, spawnSync } from "node:child_process";
import { chmodSync, copyFileSync, existsSync, mkdirSync, rmSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repo = resolve(root, "..", "..");
const exe = process.platform === "win32" ? "loams.exe" : "loams";

if (process.env.LOAMS_BUILD_ENGINE === "1") {
	execFileSync(
		"cargo",
		["build", "--release", "-p", "loams", "--features", "live,durable"],
		{ cwd: repo, stdio: "inherit" },
	);
}

let src = process.env.LOAMS_BIN;
if (!src) {
	const meta = JSON.parse(
		execFileSync("cargo", ["metadata", "--format-version", "1", "--no-deps"], {
			cwd: repo,
			maxBuffer: 256 * 1024 * 1024,
		}).toString(),
	);
	src = join(meta.target_directory, "release", exe);
}
if (!existsSync(src)) {
	console.error(
		`engine binary not found at ${src}\n` +
			"Build it: cargo build --release -p loams --features live,durable (or set LOAMS_BIN / LOAMS_BUILD_ENGINE=1).",
	);
	process.exit(1);
}

const outDir = join(root, "resources", "bin");
rmSync(outDir, { recursive: true, force: true });
mkdirSync(outDir, { recursive: true });
const dest = join(outDir, exe);
copyFileSync(src, dest);
chmodSync(dest, 0o755);

// The unstripped release binary is ~470 MB. Strip the copy, never the cargo output.
if (process.platform !== "win32") {
	const flags = process.platform === "darwin" ? ["-x"] : [];
	let stripped = false;
	for (const tool of ["strip", "llvm-strip"]) {
		const r = spawnSync(tool, [...flags, dest], { stdio: "ignore" });
		if (r.status === 0) {
			stripped = true;
			break;
		}
	}
	if (!stripped) console.warn("warning: no strip/llvm-strip found; shipping an unstripped engine");
}
const mb = (statSync(dest).size / 1048576).toFixed(1);
console.log(`engine: ${src} -> ${dest} (${mb} MB)`);

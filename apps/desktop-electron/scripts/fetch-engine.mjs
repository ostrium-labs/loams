// Copies the engine binary into resources/bin/ for electron-builder (extraResources) and strips it.
// Source: LOAMS_BIN, else <cargo target_directory>/release/loams[.exe] from `cargo metadata`.
// Set LOAMS_BUILD_ENGINE=1 to run `cargo build --release -p loams --features live,durable,live-tikv,graph` first
// (the default build has live, on the embedded store, but lacks durable and live-tikv, which the
// desktop needs for durable execution and Live on the TiKV stack, ruling T23-7, and graph, which
// serves loams.graph.v1 to the Graph page, GR1 Task 8).
// Never sets CARGO_TARGET_DIR: the shared target dir comes from the cargo config.
import { execFileSync, spawnSync } from "node:child_process";
import { chmodSync, copyFileSync, existsSync, mkdirSync, rmSync, statSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repo = resolve(root, "..", "..");

// Windows builds set this when the engine did not build or pass its smoke test: ship no engine, and
// the app shows the "not available on Windows yet" state. resources/bin stays an empty directory.
if (process.env.LOAMS_DESKTOP_NO_LOCAL_ENGINE === "1") {
	const out = join(root, "resources", "bin");
	rmSync(out, { recursive: true, force: true });
	mkdirSync(out, { recursive: true });
	console.warn("LOAMS_DESKTOP_NO_LOCAL_ENGINE=1: not bundling the local engine");
	process.exit(0);
}
const exe = process.platform === "win32" ? "loams.exe" : "loams";

if (process.env.LOAMS_BUILD_ENGINE === "1") {
	execFileSync(
		"cargo",
		["build", "--release", "-p", "loams", "--features", "live,durable,live-tikv,graph"],
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
			"Build it: cargo build --release -p loams --features live,durable,live-tikv,graph (or set LOAMS_BIN / LOAMS_BUILD_ENGINE=1).",
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

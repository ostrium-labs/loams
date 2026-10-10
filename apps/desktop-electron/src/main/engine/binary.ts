import { execFile } from "node:child_process";
import { statSync } from "node:fs";

export function findEngineBinary(
	candidates: string[],
	exists: (p: string) => boolean,
): string | null {
	return candidates.find((c) => c && exists(c)) ?? null;
}

export interface EnginePorts {
	http: number;
	flight: number;
	es: number;
	durable: number;
	live?: number;
}

export interface EngineArgsOpts {
	dataDir: string;
	ports: EnginePorts;
	/**
	 * `supported`: the binary has Live. `embedded`: it runs Live on its
	 * embedded store by default (`--live-store`, LV1 Task 23). `pd`: the
	 * TiKV stack's PD, when one is set.
	 */
	live: { supported: boolean; embedded?: boolean; pd?: string };
}

/**
 * What a `loams` binary offers for Live, read from `loams dev --help`: `none`; `tikv`, an older
 * engine whose Live needs a PD; `embedded`, Live on its embedded store only; `embedded+tikv`, the
 * embedded store or TiKV (a `live-tikv` build, ruling T23-9).
 */
export type LiveSupport = "none" | "tikv" | "embedded" | "embedded+tikv";

export function engineArgs({ dataDir, ports, live }: EngineArgsOpts): string[] {
	const a = [
		"dev",
		"--data-dir",
		dataDir,
		"--listen",
		`127.0.0.1:${ports.http}`,
		"--flight-sql-listen",
		`127.0.0.1:${ports.flight}`,
		"--es-listen",
		`127.0.0.1:${ports.es}`,
		"--no-qdrant",
		"--durable-listen",
		`127.0.0.1:${ports.durable}`,
	];
	if (live.supported) {
		if (ports.live !== undefined && (live.pd || live.embedded)) {
			a.push("--live-listen", `127.0.0.1:${ports.live}`);
			// An engine with `--live-store` selects TiKV through it (its
			// `--live-pd` is deprecated); an older one only has `--live-pd`.
			if (live.pd)
				a.push(
					...(live.embedded
						? ["--live-store", `tikv://${live.pd}`]
						: ["--live-pd", live.pd]),
				);
		} else a.push("--no-live"); // an older engine's Live needs a PD
	}
	return a;
}

export function helpSupportsLive(help: string): boolean {
	return /--(no-live|live-listen)\b/.test(help);
}

/** A flag's line in `--help` output and its description, up to the next flag; null if absent. */
function flagHelp(help: string, flag: string): string | null {
	const lines = help.split("\n");
	const at = lines.findIndex((l) => l.trimStart().startsWith(flag));
	if (at < 0) return null;
	const block = [lines[at] ?? ""];
	for (const l of lines.slice(at + 1)) {
		if (l.trimStart().startsWith("-")) break;
		block.push(l);
	}
	return block.join("\n");
}

/**
 * The binary's Live support from its `dev --help`: `none` without Live; `tikv` for an older
 * engine without `--live-store` (its Live needs a PD); else `embedded+tikv` when the
 * `--live-store` help names `tikv://` (only a `live-tikv` build's does), `embedded` otherwise.
 */
export function liveSupportFromHelp(help: string): LiveSupport {
	if (!helpSupportsLive(help)) return "none";
	const store = flagHelp(help, "--live-store");
	if (store === null) return "tikv";
	return /tikv:\/\//.test(store) ? "embedded+tikv" : "embedded";
}

const probeCache = new Map<string, LiveSupport>();

/** `<bin> dev --help` once per binary path + mtime; live is an optional cargo feature. */
export async function probeLiveSupport(bin: string): Promise<LiveSupport> {
	let key = bin;
	try {
		key = `${bin}:${statSync(bin).mtimeMs}`;
	} catch {
		return "none";
	}
	const hit = probeCache.get(key);
	if (hit !== undefined) return hit;
	const supported = await new Promise<LiveSupport>((resolve) => {
		execFile(
			bin,
			["dev", "--help"],
			{ timeout: 10_000, maxBuffer: 4 * 1024 * 1024 },
			(err, stdout, stderr) =>
				resolve(err ? "none" : liveSupportFromHelp(`${stdout}\n${stderr}`)),
		);
	});
	probeCache.set(key, supported);
	return supported;
}

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
	live: { supported: boolean; pd?: string };
}

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
		if (live.pd && ports.live !== undefined)
			a.push("--live-listen", `127.0.0.1:${ports.live}`, "--live-pd", live.pd);
		else a.push("--no-live");
	}
	return a;
}

export function helpSupportsLive(help: string): boolean {
	return /--(no-live|live-listen)\b/.test(help);
}

const probeCache = new Map<string, boolean>();

/** `<bin> dev --help` once per binary path + mtime; live is an optional cargo feature. */
export async function probeLiveSupport(bin: string): Promise<boolean> {
	let key = bin;
	try {
		key = `${bin}:${statSync(bin).mtimeMs}`;
	} catch {
		return false;
	}
	const hit = probeCache.get(key);
	if (hit !== undefined) return hit;
	const supported = await new Promise<boolean>((resolve) => {
		execFile(
			bin,
			["dev", "--help"],
			{ timeout: 10_000, maxBuffer: 4 * 1024 * 1024 },
			(err, stdout, stderr) =>
				resolve(!err && helpSupportsLive(`${stdout}\n${stderr}`)),
		);
	});
	probeCache.set(key, supported);
	return supported;
}

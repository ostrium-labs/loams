import { describe, expect, it } from "vite-plus/test";
import {
	agentdDir,
	parseDiscovery,
	readDiscovery,
} from "../src/main/agentd/discover";

const good = {
	protocol: 1,
	version: "0.2.1",
	pid: 41234,
	port: 50217,
	startedAtMs: 1791500000000,
	mode: "service",
};

describe("agentd discovery", () => {
	it("parses_a_valid_file", () => {
		expect(parseDiscovery(JSON.stringify(good))).toEqual(good);
	});

	it("rejects_malformed_files", () => {
		expect(parseDiscovery("not json")).toBeNull();
		expect(parseDiscovery("[]")).toBeNull();
		expect(parseDiscovery(JSON.stringify({ ...good, port: 0 }))).toBeNull();
		expect(parseDiscovery(JSON.stringify({ ...good, port: 70000 }))).toBeNull();
		expect(parseDiscovery(JSON.stringify({ ...good, mode: "edge" }))).toBeNull();
		expect(parseDiscovery(JSON.stringify({ ...good, pid: -1 }))).toBeNull();
		expect(parseDiscovery(JSON.stringify({ ...good, version: "" }))).toBeNull();
	});

	it("absent_when_missing", () => {
		const r = readDiscovery("/d", {
			readFile: () => {
				throw new Error("ENOENT");
			},
			pidAlive: () => true,
		});
		expect(r).toEqual({ kind: "absent" });
	});

	it("stale_discovery_is_ignored", () => {
		const r = readDiscovery("/d", {
			readFile: () => JSON.stringify(good),
			pidAlive: () => false,
		});
		expect(r).toEqual({ kind: "stale", pid: 41234 });
	});

	it("protocol_mismatch_reported", () => {
		const r = readDiscovery("/d", {
			readFile: () => JSON.stringify({ ...good, protocol: 2 }),
			pidAlive: () => true,
		});
		expect(r).toEqual({ kind: "protocol_mismatch", protocol: 2 });
	});

	it("reads_agentd_json_under_the_agentd_dir", () => {
		const seen: string[] = [];
		const dir = agentdDir("/home/u/.config/Loams Desktop");
		const r = readDiscovery(dir, {
			readFile: (p) => {
				seen.push(p);
				return JSON.stringify(good);
			},
			pidAlive: () => true,
		});
		expect(r.kind).toBe("found");
		expect(seen[0]?.replaceAll("\\", "/")).toBe(
			"/home/u/.config/Loams Desktop/agentd/agentd.json",
		);
	});
});

import { normalize, resolve } from "node:path";
import { describe, expect, it } from "vite-plus/test";
import { resolvePaths } from "../src/main/paths";

const base = {
	userData: "/home/u/.config/Loams",
	logs: "/home/u/.config/Loams/logs",
	resourcesPath: "/opt/Loams/resources",
	isPackaged: false,
	appRoot: "/repo/apps/desktop-electron",
	platform: "linux" as NodeJS.Platform,
};

// The paths are built with node:path, so on Windows they use backslashes and the dev paths, resolved
// from appRoot, gain a drive letter. Joined paths: n(); paths under the resolved repo root: r().
const n = (p: string) => normalize(p);
const r = (p: string) => resolve(p);

describe("resolvePaths", () => {
	it("packaged_paths_use_resources", () => {
		const p = resolvePaths({ ...base, isPackaged: true });
		expect(p.consoleDist).toBe(n("/opt/Loams/resources/console"));
		expect(p.engineData).toBe(n("/home/u/.config/Loams/engine"));
		expect(p.factoryDir).toBe(n("/home/u/.config/Loams/factory"));
		expect(p.serversFile).toBe(n("/home/u/.config/Loams/servers.json"));
		expect(p.logs).toBe(base.logs);
		expect(p.stacksDir).toBe(n("/opt/Loams/resources/stacks"));
		expect(p.stacksRunDir).toBe(n("/home/u/.config/Loams/stacks"));
	});

	it("dev_paths_use_repo_dist", () => {
		const p = resolvePaths(base);
		expect(p.consoleDist).toBe(r("/repo/web/apps/console/dist"));
		expect(p.stacksDir).toBe(r("/repo/deploy"));
	});

	it("engine_bin_candidates_order", () => {
		const p = resolvePaths({
			...base,
			loamsBin: "/x/loams",
			cargoTargetDir: "/cargo/target",
		});
		expect(p.engineBin).toEqual([
			"/x/loams",
			n("/opt/Loams/resources/bin/loams"),
			n("/cargo/target/release/loams"),
			n("/cargo/target/debug/loams"),
		]);
	});

	it("engine_bin_defaults_to_repo_target_and_skips_unset_override", () => {
		const p = resolvePaths(base);
		expect(p.engineBin).toEqual([
			n("/opt/Loams/resources/bin/loams"),
			r("/repo/target/release/loams"),
			r("/repo/target/debug/loams"),
		]);
	});

	it("packaged_has_no_cargo_candidates_without_target_dir", () => {
		const p = resolvePaths({ ...base, isPackaged: true });
		expect(p.engineBin).toEqual([n("/opt/Loams/resources/bin/loams")]);
	});

	it("engine_bin_uses_exe_on_win32", () => {
		const p = resolvePaths({
			...base,
			platform: "win32",
			loamsBin: "C:\\x\\loams.exe",
			cargoTargetDir: "/cargo/target",
		});
		expect(p.engineBin[0]).toBe("C:\\x\\loams.exe");
		expect(p.engineBin.slice(1).every((b) => b.endsWith("loams.exe"))).toBe(
			true,
		);
		expect(p.engineBin).toHaveLength(4);
	});

	it("agentd_bin_candidates_on_each_platform", () => {
		for (const [platform, exe] of [
			["linux", "loams-agentd"],
			["darwin", "loams-agentd"],
			["win32", "loams-agentd.exe"],
		] as const) {
			const p = resolvePaths({ ...base, platform, cargoTargetDir: "/cargo/target" });
			expect(p.agentdBin).toEqual([
				n(`/opt/Loams/resources/bin/${exe}`),
				n(`/cargo/target/release/${exe}`),
				n(`/cargo/target/debug/${exe}`),
			]);
		}
	});

	it("agentd_bin_override_first_and_packaged_has_only_the_bundle", () => {
		const p = resolvePaths({
			...base,
			platform: "win32",
			isPackaged: true,
			agentdBin: "C:\\x\\loams-agentd.exe",
		});
		expect(p.agentdBin).toEqual([
			"C:\\x\\loams-agentd.exe",
			n("/opt/Loams/resources/bin/loams-agentd.exe"),
		]);
	});
});

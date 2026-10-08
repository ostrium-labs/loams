import { describe, expect, it } from "vitest";
import { resolvePaths } from "../src/main/paths";

const base = {
	userData: "/home/u/.config/Loams",
	logs: "/home/u/.config/Loams/logs",
	resourcesPath: "/opt/Loams/resources",
	isPackaged: false,
	appRoot: "/repo/apps/desktop-electron",
	platform: "linux" as NodeJS.Platform,
};

describe("resolvePaths", () => {
	it("packaged_paths_use_resources", () => {
		const p = resolvePaths({ ...base, isPackaged: true });
		expect(p.consoleDist).toBe("/opt/Loams/resources/console");
		expect(p.engineData).toBe("/home/u/.config/Loams/engine");
		expect(p.factoryDir).toBe("/home/u/.config/Loams/factory");
		expect(p.serversFile).toBe("/home/u/.config/Loams/servers.json");
		expect(p.logs).toBe(base.logs);
		expect(p.stacksDir).toBe("/opt/Loams/resources/stacks");
	});

	it("dev_paths_use_repo_dist", () => {
		const p = resolvePaths(base);
		expect(p.consoleDist).toBe("/repo/web/apps/console/dist");
		expect(p.stacksDir).toBe("/repo/deploy");
	});

	it("engine_bin_candidates_order", () => {
		const p = resolvePaths({
			...base,
			loamsBin: "/x/loams",
			cargoTargetDir: "/cargo/target",
		});
		expect(p.engineBin).toEqual([
			"/x/loams",
			"/opt/Loams/resources/bin/loams",
			"/cargo/target/release/loams",
			"/cargo/target/debug/loams",
		]);
	});

	it("engine_bin_defaults_to_repo_target_and_skips_unset_override", () => {
		const p = resolvePaths(base);
		expect(p.engineBin).toEqual([
			"/opt/Loams/resources/bin/loams",
			"/repo/target/release/loams",
			"/repo/target/debug/loams",
		]);
	});

	it("packaged_has_no_cargo_candidates_without_target_dir", () => {
		const p = resolvePaths({ ...base, isPackaged: true });
		expect(p.engineBin).toEqual(["/opt/Loams/resources/bin/loams"]);
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
});

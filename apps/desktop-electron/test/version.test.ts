import { describe, expect, it } from "vitest";
import { VERSION_ARG, versionFromArgv } from "../src/shared/version";

describe("version argument", () => {
	it("reads_the_version_the_main_process_passed", () => {
		expect(versionFromArgv(["electron", `${VERSION_ARG}1.4.0`, "--x"])).toBe(
			"1.4.0",
		);
	});
	it("falls_back_when_missing_or_empty", () => {
		expect(versionFromArgv(["electron"])).toBe("0.0.0");
		expect(versionFromArgv([VERSION_ARG])).toBe("0.0.0");
	});
});

import { describe, expect, it } from "vitest";
import { quitSequence } from "../src/main/shell/quit";

function deps(over: Partial<Parameters<typeof quitSequence>[0]> = {}) {
	const order: string[] = [];
	return {
		order,
		d: {
			flush: async () => {
				await new Promise((r) => setTimeout(r, 5));
				order.push("flush");
			},
			stopEngine: async () => {
				order.push("engine");
			},
			hasVerifiedDownload: () => false,
			installOnQuit: async () => {
				order.push("install");
				return true;
			},
			timeoutMs: 1000,
			...over,
		},
	};
}

describe("quitSequence", () => {
	it("flushes_state_before_install_on_quit", async () => {
		const { order, d } = deps({ hasVerifiedDownload: () => true });
		await quitSequence(d);
		expect(order).toEqual(["flush", "install"]);
	});

	it("plain_quit_flushes_then_stops_the_engine", async () => {
		const { order, d } = deps();
		await quitSequence(d);
		expect(order).toEqual(["flush", "engine"]);
	});

	it("install_not_started_falls_back_to_engine_stop", async () => {
		const { order, d } = deps({
			hasVerifiedDownload: () => true,
		});
		d.installOnQuit = async () => {
			order.push("install");
			return false;
		};
		await quitSequence(d);
		expect(order).toEqual(["flush", "install", "engine"]);
	});

	it("a_hung_flush_is_bounded", async () => {
		const { order, d } = deps({
			flush: () => new Promise(() => {}),
			timeoutMs: 20,
		});
		await quitSequence(d);
		expect(order).toEqual(["engine"]);
	});

	it("a_failing_flush_still_quits", async () => {
		const { order, d } = deps({
			flush: async () => {
				throw new Error("x");
			},
		});
		await quitSequence(d);
		expect(order).toEqual(["engine"]);
	});
});

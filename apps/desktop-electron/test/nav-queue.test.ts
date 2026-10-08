import { describe, expect, it } from "vitest";
import { NavQueue } from "../src/main/shell/nav-queue";

describe("nav-queue", () => {
	it("cold_start_link_held_until_taken", () => {
		const q = new NavQueue();
		expect(q.submit("/servers")).toBe("held");
		expect(q.take()).toBe("/servers");
		expect(q.take()).toBeNull();
	});
	it("live_link_pushed_after_ready", () => {
		const q = new NavQueue();
		expect(q.take()).toBeNull();
		expect(q.submit("/data")).toBe("push");
		expect(q.take()).toBeNull();
	});
	it("reload_resets_ready", () => {
		const q = new NavQueue();
		q.take();
		q.reset();
		expect(q.submit("/factory")).toBe("held");
		expect(q.take()).toBe("/factory");
	});
	it("newer_link_replaces_pending", () => {
		const q = new NavQueue();
		q.submit("/a");
		q.submit("/b");
		expect(q.take()).toBe("/b");
	});
});

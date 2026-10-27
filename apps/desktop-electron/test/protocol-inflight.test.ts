import { describe, expect, it } from "vite-plus/test";
import { InFlight } from "../src/main/protocol/inflight";

/** A request that settles only when aborted (rejects) or when `finish` is called. */
function pending(signal: AbortSignal) {
	let finish!: (v: string) => void;
	const p = new Promise<string>((resolve, reject) => {
		finish = resolve;
		signal.addEventListener("abort", () => reject(new Error("aborted")));
	});
	return { p, finish: () => finish("done") };
}

describe("InFlight", () => {
	it("abortAll aborts every running request and waits for each to settle", async () => {
		const f = new InFlight();
		const signals: AbortSignal[] = [];
		const a = f.run((s) => {
			signals.push(s);
			return pending(s).p;
		});
		const b = f.run((s) => {
			signals.push(s);
			return pending(s).p;
		});
		expect(f.size).toBe(2);
		await f.abortAll();
		expect(signals.every((s) => s.aborted)).toBe(true);
		await expect(a).rejects.toThrow("aborted");
		await expect(b).rejects.toThrow("aborted");
		expect(f.size).toBe(0);
	});

	it("passes results through and forgets finished requests", async () => {
		const f = new InFlight();
		let finish!: () => void;
		const r = f.run((s) => {
			const x = pending(s);
			finish = x.finish;
			return x.p;
		});
		finish();
		await expect(r).resolves.toBe("done");
		await Promise.resolve();
		expect(f.size).toBe(0);
		await f.abortAll(); // nothing running: resolves at once
	});

	it("a request started after abortAll is not aborted", async () => {
		const f = new InFlight();
		await f.abortAll();
		let seen: AbortSignal | undefined;
		const r = f.run(async (s) => {
			seen = s;
			return 1;
		});
		await expect(r).resolves.toBe(1);
		expect(seen?.aborted).toBe(false);
	});
});

import { EventEmitter } from "node:events";
import { describe, expect, it, vi } from "vitest";

// Replays what mysql2's core Query emits, without a server.
const emitters: EventEmitter[] = [];
let destroyed = 0;
vi.mock("mysql2/promise", () => ({
	createConnection: async () => ({
		on() {},
		destroy: () => {
			destroyed++;
		},
		end: async () => {},
		connection: {
			query: (o: { sql: string }) => {
				const q = new EventEmitter();
				emitters.push(q);
				queueMicrotask(() => {
					if (o.sql.startsWith("INSERT")) {
						// A write: `fields` is null and the OK packet arrives as a non-array `result`; no usable `end`.
						q.emit("fields", null);
						q.emit("result", { affectedRows: 1 });
					} else {
						q.emit("fields", [{ name: "n" }]);
						for (let i = 0; i < 10; i++) q.emit("result", [i]);
						q.emit("end");
					}
				});
				return q;
			},
		},
	}),
}));

import { connectMySql } from "../src/main/sql/wesql";

describe("mysql2 stream path", () => {
	it("ok_packet_write_resolves_without_end", async () => {
		const s = await connectMySql({})();
		await expect(
			s.query("INSERT INTO t VALUES (1)", { limit: 1001 }),
		).resolves.toEqual({
			columns: [],
			rows: [],
		});
	});

	it("stops_and_drops_the_connection_at_the_limit", async () => {
		const s = await connectMySql({})();
		const before = destroyed;
		const r = await s.query("SELECT n FROM t", { limit: 4 });
		expect(r.rows).toEqual([[0], [1], [2], [3]]);
		expect(r.columns).toEqual(["n"]);
		expect(destroyed).toBe(before + 1);
		// Later statements (the ROLLBACK) on a dropped connection are no-ops, not errors.
		await expect(s.query("ROLLBACK")).resolves.toEqual({
			columns: [],
			rows: [],
		});
	});
});

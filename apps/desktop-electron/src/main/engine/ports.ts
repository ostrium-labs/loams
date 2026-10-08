// Adapted from dataelement/dsh-desktop (MIT), src/main/runtime/harness-runtime.ts.
import { createServer } from "node:net";

function reserveOne(): Promise<{ port: number; close: () => Promise<void> }> {
	return new Promise((resolve, reject) => {
		const s = createServer();
		s.once("error", reject);
		s.listen(0, "127.0.0.1", () => {
			const a = s.address();
			if (!a || typeof a === "string") {
				s.close();
				reject(new Error("no port"));
				return;
			}
			resolve({
				port: a.port,
				close: () => new Promise<void>((r) => s.close(() => r())),
			});
		});
	});
}

/** n distinct free loopback ports. All are held open until every one is picked, then released. */
export async function reservePorts(n: number): Promise<number[]> {
	const held: Awaited<ReturnType<typeof reserveOne>>[] = [];
	try {
		for (let i = 0; i < n; i++) held.push(await reserveOne());
		return held.map((h) => h.port);
	} finally {
		await Promise.all(held.map((h) => h.close()));
	}
}

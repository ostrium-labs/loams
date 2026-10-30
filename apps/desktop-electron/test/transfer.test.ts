// Transfers in main (design §85 §12, DQ1 Task 6): the event model, the
// sidecar supervisor and the service, with a fake sidecar process.
import { EventEmitter } from "node:events";
import { PassThrough } from "node:stream";
import { describe, expect, it } from "vite-plus/test";
import type {
	ServerEntry,
	TransferAgentState,
	TransferProgress,
} from "../src/shared/contracts";
import { LOG_LINES, parseEvent, TransfersModel } from "../src/main/transfer/model";
import { TransferService, type TransferKeys } from "../src/main/transfer/service";
import { NOT_BUNDLED, TransferSupervisor } from "../src/main/transfer/supervisor";

const progress = (done: number, total = 100): TransferProgress => ({
	tid: "01J0000000000000000000000T",
	direction: "upload",
	phase: "transferring",
	bytes_done: done,
	bytes_total: total,
	resumed_from: 40,
	rate_bps: 10,
	eta_s: (total - done) / 10,
	transport: "h3",
	inflight: 4,
	retries: 0,
});

describe("parseEvent", () => {
	it("protocol_parses_events", () => {
		expect(parseEvent('{"event":"ready","version":"0.1.3","install_id":"i","unfinished_uploads":[]}'))
			.toMatchObject({ event: "ready" });
		expect(parseEvent(JSON.stringify({ event: "progress", job: "j", progress: progress(1) })))
			.toMatchObject({ event: "progress" });
		expect(parseEvent("not json")).toBeNull();
		expect(parseEvent('{"event":"progress","job":"j"}')).toBeNull();
		expect(parseEvent('{"event":"surprise"}')).toBeNull();
		expect(parseEvent("[1,2]")).toBeNull();
	});
});

describe("TransfersModel", () => {
	const job = (id: string) => ({
		id,
		kind: "upload" as const,
		name: "big.bin",
		path: "/data/big.bin",
		server: "https://h:7790",
		ns: "acme",
		state: "queued" as const,
		resumed: false,
		progress: null,
		error: null,
		log: [],
	});

	it("page_shows_resume_state: unfinished uploads come back paused and resumable", () => {
		const m = new TransfersModel();
		m.apply({
			event: "ready",
			version: "0.1.3",
			install_id: "install-1",
			unfinished_uploads: [
				{ tid: "T1", path: "/data/big.bin", name: "big.bin", size: 10, server: "https://h:7790", ns: "acme" },
			],
		});
		const s = m.snapshot();
		expect(s.agent).toEqual({ phase: "ready", version: "0.1.3" });
		expect(s.jobs).toHaveLength(1);
		expect(s.jobs[0]).toMatchObject({ tid: "T1", state: "paused", resumed: true });
	});

	it("folds progress and state", () => {
		const m = new TransfersModel();
		m.add(job("j1"));
		expect(m.apply({ event: "state", job: "j1", state: "started", resumed: true })).toBe(true);
		m.apply({ event: "progress", job: "j1", progress: progress(60) });
		let j = m.snapshot().jobs[0];
		expect(j).toMatchObject({ state: "running", resumed: true, tid: "01J0000000000000000000000T" });
		expect(j?.progress?.bytes_done).toBe(60);
		m.apply({ event: "state", job: "j1", state: "failed", code: "lease_held", error: "held" });
		j = m.snapshot().jobs[0];
		expect(j?.error).toEqual({ code: "lease_held", message: "held" });
		expect(m.apply({ event: "progress", job: "nope", progress: progress(1) })).toBe(false);
	});

	it("log_view_follows_offset: pieces join across lines and the tail is bounded", () => {
		const m = new TransfersModel();
		m.add({ ...job("r1"), kind: "run" });
		m.apply({ event: "log", job: "r1", rid: "R", offset: 0, len: 6, text: "hel" + "lo\nw" });
		m.apply({ event: "log", job: "r1", rid: "R", offset: 6, len: 6, text: "orld\n" + "!" });
		const j = m.snapshot().jobs[0];
		expect(j?.log).toEqual(["hello", "world", "!"]);
		expect(j?.logOffset).toBe(12);
		const many = Array.from({ length: LOG_LINES + 10 }, (_, i) => `l${i}`).join("\n");
		m.apply({ event: "log", job: "r1", rid: "R", offset: 12, text: `\n${many}` });
		expect(m.snapshot().jobs[0]?.log.length).toBe(LOG_LINES);
	});
});

class FakeChild extends EventEmitter {
	stdin = new PassThrough();
	stdout = new PassThrough();
	stderr = new PassThrough();
	exitCode: number | null = null;
	signalCode: string | null = null;
	written: string[] = [];
	constructor() {
		super();
		this.stdin.on("data", (b: Buffer) => this.written.push(...b.toString().trim().split("\n")));
	}
	ready(): void {
		this.stdout.write('{"event":"ready","version":"0.1.3","install_id":"i","unfinished_uploads":[]}\n');
	}
	die(code: number): void {
		this.exitCode = code;
		this.emit("exit", code, null);
	}
	kill(): boolean {
		this.die(0);
		return true;
	}
}

function fakeSpawn(children: FakeChild[]) {
	return ((..._args: unknown[]) => {
		const c = new FakeChild();
		children.push(c);
		queueMicrotask(() => c.ready());
		return c;
	}) as unknown as typeof import("node:child_process").spawn;
}

describe("TransferSupervisor", () => {
	it("starts on first use and sends JSON lines", async () => {
		const children: FakeChild[] = [];
		const s = new TransferSupervisor({
			spawn: fakeSpawn(children),
			binary: () => "/bin/loams-transfer",
			stateDir: "/state",
			sleep: async () => {},
			now: () => 0,
		});
		expect(s.phase()).toEqual({ phase: "stopped" });
		await s.send({ op: "list", id: "x" });
		expect(children).toHaveLength(1);
		expect(s.phase()).toEqual({ phase: "ready", version: "0.1.3" });
		await new Promise((r) => setTimeout(r, 0));
		expect(JSON.parse(children[0]?.written[0] ?? "{}")).toEqual({ op: "list", id: "x" });
	});

	it("supervisor_restarts_with_backoff, then gives up", async () => {
		const children: FakeChild[] = [];
		const waits: number[] = [];
		const phases: TransferAgentState[] = [];
		const s = new TransferSupervisor({
			spawn: fakeSpawn(children),
			binary: () => "/bin/loams-transfer",
			stateDir: "/state",
			sleep: async (ms) => {
				waits.push(ms);
			},
			now: () => 1000,
		});
		s.on("phase", (p: TransferAgentState) => phases.push(p));
		await s.ensure();
		for (let i = 0; i < 6; i++) {
			const c = children[children.length - 1];
			c?.die(1);
			await new Promise((r) => setTimeout(r, 5));
		}
		expect(waits).toEqual([1000, 2000, 4000, 8000, 16000]);
		expect(s.phase().phase).toBe("failed");
	});

	it("says when the sidecar is not bundled", async () => {
		const s = new TransferSupervisor({
			spawn: fakeSpawn([]),
			binary: () => null,
			stateDir: "/state",
			sleep: async () => {},
			now: () => 0,
		});
		await expect(s.ensure()).rejects.toThrow(NOT_BUNDLED);
		expect(s.phase()).toEqual({ phase: "failed", reason: NOT_BUNDLED });
	});
});

describe("TransferService", () => {
	const servers: ServerEntry[] = [
		{ id: "s1", name: "lab", kind: "remote", url: "https://lab.tail:7790" },
		{ id: "demo", name: "Demo", kind: "demo", url: "https://demo" },
	];
	function setup() {
		const sent: Record<string, unknown>[] = [];
		const sup = Object.assign(new EventEmitter(), {
			send: async (c: Record<string, unknown>) => {
				sent.push(c);
			},
			phase: (): TransferAgentState => ({ phase: "stopped" }),
		});
		const store = new Map<string, { token: string; settings: object }>();
		const keys: TransferKeys = {
			persistent: true,
			get: (id) => (store.get(id) as ReturnType<TransferKeys["get"]>) ?? null,
			set: (id, token, settings) => store.set(id, { token, settings }),
		};
		let n = 0;
		const svc = new TransferService({
			supervisor: sup as never,
			servers: () => servers,
			keys,
			newId: () => `id${++n}`,
		});
		return { svc, sent, sup };
	}

	it("needs a key, then sends the upload with the server's trust", async () => {
		const { svc, sent } = setup();
		const t = { serverId: "s1", ns: "acme" };
		const r = await svc.upload(t, "/data/big.bin");
		expect(r).toMatchObject({ ok: false, code: "no_token" });
		expect(svc.setToken("s1", "short", {})).toMatchObject({ ok: false });
		expect(svc.setToken("s1", "k".repeat(20), { pin: "AB".repeat(32), mode: "h3" }).ok).toBe(true);
		const ok = await svc.upload(t, "/data/big.bin");
		expect(ok.ok).toBe(true);
		expect(sent[0]).toMatchObject({
			op: "upload",
			path: "/data/big.bin",
			ns: "acme",
			token: "k".repeat(20),
			server: { base: "https://lab.tail:7790", mode: "h3", pin: "ab".repeat(32) },
		});
		expect(svc.snapshot().jobs[0]).toMatchObject({ name: "big.bin", state: "queued" });
	});

	it("refuses the demo server and bad namespaces", async () => {
		const { svc } = setup();
		svc.setToken("s1", "k".repeat(20), {});
		expect(await svc.upload({ serverId: "demo", ns: "acme" }, "/x")).toMatchObject({ ok: false, code: "unsupported" });
		expect(await svc.upload({ serverId: "s1", ns: "../etc" }, "/x")).toMatchObject({ ok: false, code: "invalid" });
	});

	it("resume sends the same command again under a new job", async () => {
		const { svc, sent, sup } = setup();
		svc.setToken("s1", "k".repeat(20), {});
		const r = await svc.upload({ serverId: "s1", ns: "acme" }, "/data/big.bin");
		if (!r.ok) throw new Error("upload");
		sup.emit("event", { event: "state", job: r.value.id, state: "paused" });
		const again = await svc.resume(r.value.id);
		expect(again.ok).toBe(true);
		expect(sent.filter((c) => c.op === "upload")).toHaveLength(2);
		expect(svc.snapshot().jobs).toHaveLength(1);
	});

	it("startRun follows the run the server queued", async () => {
		const { svc, sent, sup } = setup();
		svc.setToken("s1", "k".repeat(20), {});
		const p = svc.startRun({ serverId: "s1", ns: "acme" }, "ci", { REF: "main" });
		await new Promise((r) => setTimeout(r, 0));
		const req = sent.find((c) => c.op === "run.start");
		expect(req).toMatchObject({ job: "ci", params: { REF: "main" } });
		sup.emit("event", { event: "reply", id: req?.id, run: { rid: "R1" } });
		const r = await p;
		expect(r).toMatchObject({ ok: true, value: { kind: "run", rid: "R1" } });
		expect(sent.find((c) => c.op === "run.tail")).toMatchObject({ rid: "R1" });
	});
});

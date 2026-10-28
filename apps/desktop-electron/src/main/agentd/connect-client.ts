// The real AgentdClient for Electron main (0.1.0 release fix B9): it talks to the
// running daemon's loopback `loams.agentd.v1` listener (`connectPort` in
// `agentd.json`) with the token only main holds, as studio-watch.ts does.
//
// The daemon's Connect surface has no twin of the ndjson daemon-control calls the
// status panel was written against (`DaemonStatus`, `WatchDaemonStatus`, `Drain`,
// `ListSessions`, `AttachSession`), so this client maps them:
//   hello / DaemonStatus  StudioService.GetHost (version, mode, pid, start time);
//   WatchDaemonStatus     GetHost polled, re-reading the discovery file and token
//                         each time, so a restarted daemon (new port, new token) is
//                         followed and a lost one reported as `disconnected`;
//   Shutdown              SIGTERM to the daemon's pid (the daemon stops cleanly on
//                         it), then waits for the process to exit;
//   Drain                 nothing to do: the daemon finishes its turns on SIGTERM;
//   ListSessions          no sessions (Studio lists threads itself);
//   AttachSession         `unknown_method`.
// No electron import; setup.ts wires it.
import { createClient, type Transport } from "@connectrpc/connect";
import { createConnectTransport } from "@connectrpc/connect-web";
import {
	DaemonMode as ProtoDaemonMode,
	StudioService,
} from "@loams/proto/agentd";
import type { AgentdClient, ConnectFn, Subscription } from "./client";
import { type AgentdTarget, agentdDir, readAgentdTarget } from "./discover";
import {
	AGENTD_PROTOCOL,
	AgentdError,
	type DaemonMode,
	type DaemonStatus,
	type HelloReply,
} from "./types";

export interface ConnectClientDeps {
	/** The daemon's listener and token, read fresh on each call; null while none runs. */
	target?: (dataDir: string) => AgentdTarget | null;
	fetch?: typeof fetch;
	/** How often WatchDaemonStatus asks the daemon (ms). */
	pollMs?: number;
	/** How long one GetHost may take (ms). */
	timeoutMs?: number;
	/** `process.kill`, injectable for tests. */
	kill?: (pid: number, signal?: NodeJS.Signals | 0) => void;
}

const SHUTDOWN_WAIT_MS = 15_000;

function modeOf(m: ProtoDaemonMode): DaemonMode {
	return m === ProtoDaemonMode.SERVICE ? "service" : "child";
}

/** A Connect transport to the daemon's loopback listener, with the token only main holds. */
export function transportFor(
	t: AgentdTarget,
	doFetch: typeof fetch = (i, init) => globalThis.fetch(i, init),
): Transport {
	return createConnectTransport({
		baseUrl: t.url,
		useBinaryFormat: true,
		fetch: (input, init) => {
			const headers = new Headers(init?.headers);
			headers.set("authorization", t.token.bearer());
			return doFetch(input, { ...init, headers });
		},
	});
}

async function getHost(
	t: AgentdTarget,
	doFetch: typeof fetch,
	timeoutMs: number,
): Promise<HelloReply> {
	const client = createClient(StudioService, transportFor(t, doFetch));
	const r = await client.getHost({}, { timeoutMs });
	const h = r.host;
	if (!h)
		throw new AgentdError("unavailable", "The daemon did not describe itself");
	if (h.protocol !== AGENTD_PROTOCOL)
		throw new AgentdError(
			"protocol_mismatch",
			`The daemon speaks protocol ${h.protocol}`,
		);
	return {
		protocol: h.protocol,
		version: h.version,
		buildSha: h.buildSha,
		mode: modeOf(h.mode),
		pid: h.pid,
		startedAtMs: Number(h.startedAtMs),
		role: "owner",
		features: [...h.features],
	};
}

function statusOf(h: HelloReply): DaemonStatus {
	return {
		mode: h.mode,
		version: h.version,
		draining: false,
		runningTurns: 0,
		pendingApprovals: 0,
		engine: { state: "stopped" },
	};
}

/** A ConnectFn over the daemon's `loams.agentd.v1` listener. */
export function connectAgentd(deps: ConnectClientDeps = {}): ConnectFn {
	const target =
		deps.target ?? ((dataDir: string) => readAgentdTarget(agentdDir(dataDir)));
	const doFetch: typeof fetch =
		deps.fetch ?? ((input, init) => globalThis.fetch(input, init));
	const pollMs = deps.pollMs ?? 5_000;
	const timeoutMs = deps.timeoutMs ?? 5_000;
	const kill = deps.kill ?? ((pid, signal) => void process.kill(pid, signal));

	return async ({ dataDir }) => {
		const first = target(dataDir);
		if (!first)
			throw new AgentdError("unavailable", "The daemon is not running");
		let hello: HelloReply;
		try {
			hello = await getHost(first, doFetch, timeoutMs);
		} catch (e) {
			if (e instanceof AgentdError) throw e;
			throw new AgentdError("unavailable", "The daemon did not answer");
		}

		const listeners = {
			disconnected: new Set<() => void>(),
			reconnected: new Set<() => void>(),
		};
		const timers = new Set<ReturnType<typeof setInterval>>();
		let closed = false;
		let lost = false;

		const probe = async (): Promise<HelloReply | null> => {
			const t = target(dataDir);
			if (!t) return null;
			try {
				return await getHost(t, doFetch, timeoutMs);
			} catch {
				return null;
			}
		};

		const alive = (pid: number): boolean => {
			try {
				kill(pid, 0);
				return true;
			} catch (e) {
				return (e as NodeJS.ErrnoException).code === "EPERM";
			}
		};

		const client: AgentdClient = {
			get hello() {
				return hello;
			},
			async call(method) {
				if (closed)
					throw new AgentdError("unavailable", "The daemon is not running");
				switch (method) {
					case "DaemonStatus": {
						const h = await probe();
						if (!h)
							throw new AgentdError("unavailable", "The daemon is not running");
						hello = h;
						return statusOf(h) as never;
					}
					case "ListSessions":
						return [] as never;
					case "Drain":
						return null as never;
					case "Shutdown": {
						const pid = hello.pid;
						try {
							kill(pid, "SIGTERM");
						} catch {
							return null as never;
						}
						const deadline = Date.now() + SHUTDOWN_WAIT_MS;
						while (alive(pid) && Date.now() < deadline)
							await new Promise((r) => setTimeout(r, 100));
						return null as never;
					}
					default:
						throw new AgentdError("unknown_method", `${method} is not served`);
				}
			},
			subscribe(_method, _params, onItem): Subscription {
				let resolveDone: () => void = () => {};
				const done = new Promise<void>((r) => {
					resolveDone = r;
				});
				onItem(statusOf(hello) as never);
				let busy = false;
				const timer = setInterval(async () => {
					if (busy || closed) return;
					busy = true;
					try {
						const h = await probe();
						if (closed) return;
						if (!h) {
							if (!lost) {
								lost = true;
								for (const cb of listeners.disconnected) cb();
							}
							return;
						}
						hello = h;
						if (lost) {
							lost = false;
							for (const cb of listeners.reconnected) cb();
						}
						onItem(statusOf(h) as never);
					} finally {
						busy = false;
					}
				}, pollMs);
				timers.add(timer);
				return {
					done,
					cancel() {
						clearInterval(timer);
						timers.delete(timer);
						resolveDone();
					},
				};
			},
			on(event, cb) {
				listeners[event].add(cb);
				return () => void listeners[event].delete(cb);
			},
			async close() {
				closed = true;
				for (const t of timers) clearInterval(t);
				timers.clear();
			},
		};
		return client;
	};
}

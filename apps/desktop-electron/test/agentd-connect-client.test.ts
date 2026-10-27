import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { create, toBinary } from "@bufbuild/protobuf";
import { DaemonMode, GetHostResponseSchema } from "@loams/proto/agentd";
import { afterEach, describe, expect, it } from "vite-plus/test";
import { AgentdToken } from "../src/main/agentd/client";
import { connectAgentd } from "../src/main/agentd/connect-client";
import type { AgentdTarget } from "../src/main/agentd/discover";
import { createAgentdLauncher } from "../src/main/agentd/launch";
import { AgentdError } from "../src/main/agentd/types";

const target: AgentdTarget = {
	url: "http://127.0.0.1:40001",
	token: new AgentdToken("secret-token"),
};

function hostFetch(seen: { url: string; auth: string | null }[], up: () => boolean): typeof fetch {
	return (async (input: Parameters<typeof fetch>[0], init?: RequestInit) => {
		const headers = new Headers(init?.headers);
		seen.push({ url: String(input), auth: headers.get("authorization") });
		if (!up()) throw new TypeError("fetch failed");
		const body = toBinary(
			GetHostResponseSchema,
			create(GetHostResponseSchema, {
				host: {
					protocol: 1,
					version: "0.1.0",
					buildSha: "abc",
					mode: DaemonMode.CHILD,
					pid: 4242,
					startedAtMs: 1_000n,
					features: ["studio.v1"],
				},
			}),
		);
		return new Response(body, { status: 200, headers: { "content-type": "application/proto" } });
	}) as typeof fetch;
}

describe("agentd connect client", () => {
	it("unavailable_without_a_daemon", async () => {
		const connect = connectAgentd({ target: () => null });
		await expect(connect({ dataDir: "/d", clientVersion: "0.1.0" })).rejects.toMatchObject({
			code: "unavailable",
		});
	});

	it("hello_from_get_host_with_the_bearer_token", async () => {
		const seen: { url: string; auth: string | null }[] = [];
		const connect = connectAgentd({ target: () => target, fetch: hostFetch(seen, () => true) });
		const c = await connect({ dataDir: "/d", clientVersion: "0.1.0" });
		expect(c.hello).toMatchObject({ version: "0.1.0", mode: "child", pid: 4242, startedAtMs: 1000 });
		expect(seen[0]).toEqual({
			url: "http://127.0.0.1:40001/loams.agentd.v1.StudioService/GetHost",
			auth: "Bearer secret-token",
		});
		expect(await c.call("ListSessions", {})).toEqual([]);
		await expect(c.call("AttachSession", { sessionId: "x" })).rejects.toBeInstanceOf(AgentdError);
		await c.close();
	});

	it("unavailable_when_the_daemon_does_not_answer", async () => {
		const connect = connectAgentd({ target: () => target, fetch: hostFetch([], () => false) });
		await expect(connect({ dataDir: "/d", clientVersion: "0.1.0" })).rejects.toMatchObject({
			code: "unavailable",
		});
	});

	it("watch_reports_disconnect_and_reconnect", async () => {
		let up = true;
		const connect = connectAgentd({
			target: () => target,
			fetch: hostFetch([], () => up),
			pollMs: 10,
		});
		const c = await connect({ dataDir: "/d", clientVersion: "0.1.0" });
		const events: string[] = [];
		c.on("disconnected", () => events.push("down"));
		c.on("reconnected", () => events.push("up"));
		let items = 0;
		const sub = c.subscribe("WatchDaemonStatus", {}, () => items++);
		up = false;
		await new Promise((r) => setTimeout(r, 60));
		up = true;
		await new Promise((r) => setTimeout(r, 60));
		sub.cancel();
		await c.close();
		expect(events).toEqual(["down", "up"]);
		expect(items).toBeGreaterThan(1);
	});

	it("shutdown_signals_the_daemon", async () => {
		const sent: (string | number | undefined)[] = [];
		const connect = connectAgentd({
			target: () => target,
			fetch: hostFetch([], () => true),
			kill: (pid, signal) => {
				sent.push(signal);
				expect(pid).toBe(4242);
				if (signal === 0) throw Object.assign(new Error("gone"), { code: "ESRCH" });
			},
		});
		const c = await connect({ dataDir: "/d", clientVersion: "0.1.0" });
		await c.call("Shutdown", {});
		expect(sent).toEqual(["SIGTERM", 0]);
	});
});

describe.skipIf(process.platform === "win32")("agentd launcher", () => {
	let dir: string;
	afterEach(() => rmSync(dir, { recursive: true, force: true }));

	it("not_bundled_is_reported", async () => {
		dir = mkdtempSync(join(tmpdir(), "agentd-launch-"));
		const l = createAgentdLauncher({ dir: join(dir, "agentd"), logsDir: dir, candidates: [join(dir, "none")] });
		expect(l.binary()).toBeNull();
		await expect(l.launch("child")).rejects.toMatchObject({ code: "unavailable" });
	});

	it("child_runs_until_discovery_then_is_killed", async () => {
		dir = mkdtempSync(join(tmpdir(), "agentd-launch-"));
		const bin = join(dir, "loams-agentd");
		// A stand-in daemon: records its arguments, writes the discovery file, waits.
		writeFileSync(
			bin,
			`#!/bin/sh
echo "$@" > "${dir}/args"
cfgdir=$(dirname "$4")
printf '{"protocol":1,"version":"0.1.0","pid":%d,"port":40000,"connectPort":40001,"startedAtMs":1,"mode":"child"}' $$ > "$cfgdir/agentd.json"
exec sleep 30
`,
		);
		chmodSync(bin, 0o755);
		const l = createAgentdLauncher({
			dir: join(dir, "agentd"),
			logsDir: join(dir, "logs"),
			candidates: [join(dir, "missing"), bin],
			readyTimeoutMs: 5_000,
		});
		await l.launch("child");
		const cfg = join(dir, "agentd", "config.toml");
		expect(readFileSync(join(dir, "args"), "utf8").trim()).toBe(`run --child --config ${cfg}`);
		expect(readFileSync(cfg, "utf8")).toBe("");
		l.killChild();
	});

	it("service_installs_the_runtime_then_the_service", async () => {
		dir = mkdtempSync(join(tmpdir(), "agentd-launch-"));
		const bin = join(dir, "bin", "loams-agentd");
		const calls: string[][] = [];
		const l = createAgentdLauncher({
			dir: join(dir, "agentd"),
			logsDir: dir,
			candidates: [bin],
			exists: (p) => p === bin,
			readyTimeoutMs: 300,
			run: async (_file, args) => {
				calls.push(args);
			},
		});
		// No discovery file appears, so it times out after both commands ran.
		await expect(l.launch("service")).rejects.toMatchObject({ code: "unavailable" });
		const cfg = join(dir, "agentd", "config.toml");
		expect(calls).toEqual([
			["install-runtime", "--from", join(dir, "bin"), "--to", join(dir, "agentd", "runtime")],
			["service", "install", "--config", cfg],
		]);
	});
});

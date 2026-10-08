import { existsSync, mkdtempSync, readdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { ServerRegistry } from "../src/main/servers/registry";

const fresh = () => join(mkdtempSync(join(tmpdir(), "reg-")), "servers.json");

describe("ServerRegistry", () => {
	it("local_entry_always_present_and_not_removable", () => {
		const r = new ServerRegistry(fresh(), { devDemo: false });
		expect(r.list().servers.map((s) => s.id)).toEqual(["local"]);
		expect(r.active().id).toBe("local");
		expect(r.active().url).toBe("");
		const res = r.remove("local");
		expect(res.ok).toBe(false);
		expect(r.list().servers[0]?.id).toBe("local");
	});
	it("demo_only_when_dev", () => {
		const r = new ServerRegistry(fresh(), { devDemo: true });
		expect(r.list().servers.find((s) => s.id === "demo")?.url).toBe(
			"http://127.0.0.1:8084",
		);
	});
	it("rejects_http_non_loopback", () => {
		const r = new ServerRegistry(fresh(), { devDemo: false });
		const res = r.add({ name: "x", kind: "remote", url: "http://example.com" });
		expect(res).toMatchObject({ ok: false, code: "insecure_url" });
		expect(
			r.add({ name: "x", kind: "remote", url: "http://localhost:9" }).ok,
		).toBe(true);
		expect(
			r.add({ name: "x", kind: "remote", url: "ftp://example.com" }).ok,
		).toBe(false);
	});
	it("stores_origin_only", () => {
		const r = new ServerRegistry(fresh(), { devDemo: false });
		const res = r.add({
			name: "p",
			kind: "remote",
			url: "https://loams.example.com/some/path?q=1#h",
		});
		expect(res.ok && res.value.url).toBe("https://loams.example.com");
	});
	it("persists_and_activates", () => {
		const f = fresh();
		const r = new ServerRegistry(f, { devDemo: false });
		const a = r.add({ name: "p", kind: "remote", url: "https://a.example" });
		if (!a.ok) throw new Error("add");
		expect(r.activate(a.value.id).ok).toBe(true);
		const r2 = new ServerRegistry(f, { devDemo: false });
		expect(r2.active().url).toBe("https://a.example");
		expect(r2.activate("nope").ok).toBe(false);
	});
	it("corrupt_file_recovers", () => {
		const f = fresh();
		writeFileSync(f, "{not json");
		const r = new ServerRegistry(f, { devDemo: false });
		expect(r.list().servers.map((s) => s.id)).toEqual(["local"]);
		const files = readdirSync(join(f, ".."));
		expect(files.some((n) => n.startsWith("servers.json.corrupt-"))).toBe(true);
		expect(existsSync(f)).toBe(false);
	});
	it("atomic_write_survives_crash", () => {
		const f = fresh();
		const r = new ServerRegistry(f, { devDemo: false });
		r.add({ name: "keep", kind: "remote", url: "https://keep.example" });
		// crash mid-write: temp file left behind with partial content, no rename
		writeFileSync(`${f}.tmp`, '{"v":1,"servers":[');
		const r2 = new ServerRegistry(f, { devDemo: false });
		expect(r2.list().servers.map((s) => s.url)).toContain(
			"https://keep.example",
		);
		// a later write replaces the stale temp
		expect(
			r2.add({ name: "n", kind: "remote", url: "https://n.example" }).ok,
		).toBe(true);
		expect(
			new ServerRegistry(f, { devDemo: false }).list().servers,
		).toHaveLength(3);
	});
});

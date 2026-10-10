import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { ProviderConfigs } from "../src/main/agent/providers/presets";
import { ChatService } from "../src/main/agent/service";
import { ChatStore } from "../src/main/agent/store";
import { ToolRegistry } from "../src/main/agent/tools";
import { Vault, type VaultCrypto } from "../src/main/factory/vault";

const xor: VaultCrypto = {
	available: () => true,
	encrypt: (s) => Buffer.from(Buffer.from(s).map((b) => b ^ 0x5a)),
	decrypt: (b) => Buffer.from(Buffer.from(b).map((x) => x ^ 0x5a)).toString(),
};
const KEY = "sk-test-secret-1234567890";

function setup(
	fetchImpl: (url: string, init: RequestInit) => Promise<Response>,
) {
	const dir = mkdtempSync(join(tmpdir(), "agent-test-"));
	const configs = new ProviderConfigs(
		join(dir, "agent", "providers.json"),
		new Vault(join(dir, "credentials.bin"), xor),
		fetchImpl,
	);
	const svc = new ChatService({
		store: new ChatStore(join(dir, "chats")),
		configs,
		tools: new ToolRegistry(),
		emit: () => {},
		now: Date.now,
	});
	return { svc, configs };
}

const sse = (text: string) =>
	`data: ${JSON.stringify({ choices: [{ delta: { content: text } }] })}\n\n`;

describe("chat.testProvider", () => {
	it("test_provider_ok_stops_after_first_text", async () => {
		let pulls = 0;
		let cancelled = false;
		const enc = new TextEncoder();
		const { svc, configs } = setup(async () => {
			// An endless stream: the test only ends if the reader stops early.
			const body = new ReadableStream<Uint8Array>({
				pull(c) {
					pulls++;
					c.enqueue(enc.encode(sse(`w${pulls} `)));
				},
				cancel() {
					cancelled = true;
				},
			});
			return new Response(body, {
				headers: { "content-type": "text/event-stream" },
			});
		});
		configs.configure("deepseek", { model: "deepseek-chat", apiKey: KEY });
		const r = await svc.testProvider("deepseek");
		expect(r.ok && r.value.model).toBe("deepseek-chat");
		expect(cancelled).toBe(true);
		expect(pulls).toBeLessThan(5);
	});

	it("test_provider_fails_when_no_text_arrives", async () => {
		const { svc, configs } = setup(
			async () =>
				new Response(
					`data: ${JSON.stringify({ choices: [{ delta: {}, finish_reason: "length" }] })}\n\ndata: [DONE]\n\n`,
					{ headers: { "content-type": "text/event-stream" } },
				),
		);
		configs.configure("deepseek", { model: "deepseek-chat", apiKey: KEY });
		const r = await svc.testProvider("deepseek");
		expect(!r.ok && r.code).toBe("test_failed");
	});

	it("test_provider_error_is_redacted", async () => {
		const { svc, configs } = setup(
			async () =>
				new Response(JSON.stringify({ error: { message: `bad key ${KEY}` } }), {
					status: 401,
				}),
		);
		configs.configure("deepseek", { model: "deepseek-chat", apiKey: KEY });
		const r = await svc.testProvider("deepseek");
		expect(r.ok).toBe(false);
		expect(JSON.stringify(r)).not.toContain(KEY);
		expect(!r.ok && r.code).toBe("test_failed");
		expect(!r.ok && r.message).toMatch(/401/);
	});

	it("test_provider_unconfigured_and_unknown", async () => {
		const { svc } = setup(async () => new Response("", { status: 500 }));
		const a = await svc.testProvider("openai");
		expect(!a.ok && a.code).toBe("unconfigured");
		const b = await svc.testProvider("nope");
		expect(!b.ok && b.code).toBe("unknown_provider");
	});
});

import { existsSync, mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { inspect } from "node:util";
import { describe, expect, it } from "vitest";
import { Secret, Vault, type VaultCrypto } from "../src/main/factory/vault";

const xor = (available: boolean): VaultCrypto => ({
	available: () => available,
	encrypt: (s) => Buffer.from(Buffer.from(s).map((b) => b ^ 0x5a)),
	decrypt: (b) => Buffer.from(Buffer.from(b).map((x) => x ^ 0x5a)).toString(),
});
const dir = () => mkdtempSync(join(tmpdir(), "vault-"));

describe("vault", () => {
	it("round_trips_encrypted_and_hides_plaintext", () => {
		const file = join(dir(), "v.json");
		new Vault(file, xor(true)).set("forgejo", "https://f.example", {
			token: "tok-ABC",
		});
		expect(readFileSync(file, "utf8")).not.toContain("tok-ABC");
		const again = new Vault(file, xor(true)).get("forgejo");
		expect(again?.url).toBe("https://f.example");
		expect(again?.fields["token"]?.reveal()).toBe("tok-ABC");
	});

	it("vault_session_only_when_no_backend", () => {
		const file = join(dir(), "v.json");
		const v = new Vault(file, xor(false));
		v.set("forgejo", "https://f.example", { token: "tok-ABC" });
		expect(v.persistent).toBe(false);
		expect(v.get("forgejo")?.fields["token"]?.reveal()).toBe("tok-ABC");
		expect(existsSync(file)).toBe(false);
	});

	it("secret_never_stringifies", () => {
		const s = new Secret("hunter2");
		expect(`${s}`).toBe("[redacted]");
		expect(JSON.stringify({ s })).toBe('{"s":"[redacted]"}');
		expect(inspect(s)).toBe("[redacted]");
		expect(s.reveal()).toBe("hunter2");
	});

	it("remove_deletes_entry", () => {
		const file = join(dir(), "v.json");
		const v = new Vault(file, xor(true));
		v.set("zulip", "https://z", { apiKey: "k" });
		v.remove("zulip");
		expect(new Vault(file, xor(true)).get("zulip")).toBeUndefined();
	});
});

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

	it("undecryptable_entry_is_locked_and_kept_on_save", () => {
		const file = join(dir(), "v.json");
		const v1 = new Vault(file, xor(true));
		v1.set("forgejo", "https://f.example", { token: "tok-A" });
		v1.set("zulip", "https://z.example", { apiKey: "k" });
		const before = JSON.parse(readFileSync(file, "utf8")) as Record<
			string,
			string
		>;
		// A keychain that can no longer open the forgejo entry.
		const flaky: VaultCrypto = {
			...xor(true),
			decrypt: (b) => {
				const s = xor(true).decrypt(b);
				if (s.includes("tok-A")) throw new Error("bad key");
				return s;
			},
		};
		const v2 = new Vault(file, flaky);
		expect(v2.get("forgejo")).toBeUndefined();
		expect(v2.isLocked("forgejo")).toBe(true);
		expect(v2.isLocked("zulip")).toBe(false);
		// Saving another entry must not drop the locked ciphertext.
		v2.set("plane", "https://p.example", { apiKey: "p" });
		const after = JSON.parse(readFileSync(file, "utf8")) as Record<
			string,
			string
		>;
		expect(after.forgejo).toBe(before.forgejo);
		// With the old key it opens again.
		expect(
			new Vault(file, xor(true)).get("forgejo")?.fields.token?.reveal(),
		).toBe("tok-A");
	});

	it("locked_entry_is_replaced_by_set_and_dropped_by_remove", () => {
		const file = join(dir(), "v.json");
		new Vault(file, xor(true)).set("forgejo", "https://f", { token: "t" });
		const broken: VaultCrypto = {
			...xor(true),
			decrypt: () => {
				throw new Error("x");
			},
		};
		const v = new Vault(file, broken);
		expect(v.isLocked("forgejo")).toBe(true);
		v.set("forgejo", "https://g", { token: "new" });
		expect(v.isLocked("forgejo")).toBe(false);
		expect(v.get("forgejo")?.url).toBe("https://g");
		const w = new Vault(file, broken);
		expect(w.isLocked("forgejo")).toBe(true);
		w.remove("forgejo");
		expect(w.isLocked("forgejo")).toBe(false);
		expect(JSON.parse(readFileSync(file, "utf8"))).toEqual({});
	});
});

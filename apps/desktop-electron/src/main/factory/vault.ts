import { mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";
import { inspect } from "node:util";
import type { FactoryAppId } from "../../shared/contracts";

/** A credential value. It never serialises, logs or inspects as its content. */
export class Secret {
	readonly #v: string;
	constructor(v: string) {
		this.#v = v;
	}
	reveal(): string {
		return this.#v;
	}
	toString(): string {
		return "[redacted]";
	}
	toJSON(): string {
		return "[redacted]";
	}
	[inspect.custom](): string {
		return "[redacted]";
	}
}

export interface VaultCrypto {
	available(): boolean;
	encrypt(s: string): Buffer;
	decrypt(b: Buffer): string;
}

/** A factory app, or an agent provider's key (D675: `agent:<provider>`). */
export type VaultKey = FactoryAppId | `agent:${string}`;

export interface VaultEntry {
	url: string;
	fields: Record<string, Secret>;
}

/**
 * Encrypted per-app credentials. The file is a JSON map `app -> base64(ciphertext)`.
 * Without an encryption backend nothing touches the disk (`persistent` is false).
 */
export class Vault {
	readonly persistent: boolean;
	readonly #entries = new Map<string, VaultEntry>();
	/** Entries this keychain cannot open: kept verbatim (base64 ciphertext) until replaced. */
	readonly #locked = new Map<string, string>();

	constructor(
		private readonly file: string,
		private readonly crypto: VaultCrypto,
	) {
		this.persistent = crypto.available();
		if (this.persistent) this.#load();
	}

	get(app: VaultKey): VaultEntry | undefined {
		return this.#entries.get(app);
	}

	/** True when the entry exists on disk but could not be decrypted (keychain changed). */
	isLocked(app: VaultKey): boolean {
		return this.#locked.has(app);
	}

	set(app: VaultKey, url: string, fields: Record<string, string>): void {
		const wrapped: Record<string, Secret> = {};
		for (const [k, v] of Object.entries(fields)) wrapped[k] = new Secret(v);
		this.#locked.delete(app);
		this.#entries.set(app, { url, fields: wrapped });
		this.#save();
	}

	remove(app: VaultKey): void {
		const had = this.#entries.delete(app);
		if (this.#locked.delete(app) || had) this.#save();
	}

	#load(): void {
		let map: Record<string, string>;
		try {
			map = JSON.parse(readFileSync(this.file, "utf8")) as Record<
				string,
				string
			>;
		} catch {
			return;
		}
		for (const [app, b64] of Object.entries(map)) {
			if (typeof b64 !== "string") continue;
			try {
				const raw = JSON.parse(
					this.crypto.decrypt(Buffer.from(b64, "base64")),
				) as { url: string; fields: Record<string, string> };
				const fields: Record<string, Secret> = {};
				for (const [k, v] of Object.entries(raw.fields))
					fields[k] = new Secret(v);
				this.#entries.set(app, { url: raw.url, fields });
			} catch {
				// An undecryptable entry (changed keychain) is kept as-is and reported
				// locked: the old keychain may come back, and saving must not lose it.
				this.#locked.set(app, b64);
			}
		}
	}

	#save(): void {
		if (!this.persistent) return;
		const map: Record<string, string> = Object.fromEntries(this.#locked);
		for (const [app, e] of this.#entries) {
			const plain: Record<string, string> = {};
			for (const [k, s] of Object.entries(e.fields)) plain[k] = s.reveal();
			map[app] = this.crypto
				.encrypt(JSON.stringify({ url: e.url, fields: plain }))
				.toString("base64");
		}
		mkdirSync(dirname(this.file), { recursive: true });
		const tmp = `${this.file}.tmp`;
		writeFileSync(tmp, JSON.stringify(map), { mode: 0o600 });
		renameSync(tmp, this.file);
	}
}

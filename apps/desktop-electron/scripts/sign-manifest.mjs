#!/usr/bin/env node
// Usage: LOAMS_UPDATE_SIGNING_KEY=<32-byte hex seed> node sign-manifest.mjs <latest*.yml>
// Writes <yml>.sig (base64 Ed25519 signature over the file bytes).
import { createPrivateKey, sign } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";

const file = process.argv[2];
const keyHex = process.env.LOAMS_UPDATE_SIGNING_KEY ?? "";
if (!file || !/^[0-9a-fA-F]{64}$/.test(keyHex)) {
	console.error(
		"usage: LOAMS_UPDATE_SIGNING_KEY=<64 hex chars> sign-manifest.mjs <latest*.yml>",
	);
	process.exit(2);
}
// PKCS#8 prefix for an Ed25519 seed.
const der = Buffer.concat([
	Buffer.from("302e020100300506032b657004220420", "hex"),
	Buffer.from(keyHex, "hex"),
]);
const key = createPrivateKey({ key: der, format: "der", type: "pkcs8" });
const sig = sign(null, readFileSync(file), key).toString("base64");
writeFileSync(`${file}.sig`, `${sig}\n`);
console.log(`signed ${file}`);
